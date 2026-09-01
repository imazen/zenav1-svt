//! Differential parity for the CDEF SEARCH signal derivation of
//! `Source/Lib/Codec/enc_mode_config.c`: the `cdef_search_level` ladders of
//! `svt_aom_sig_deriv_multi_processes_default` (`:2083`) and
//! `..._allintra` (`:2396`), and the `set_cdef_search_controls` table
//! (`:891`) both feed.
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4). `set_cdef_search_controls`
//! is file-`static` and the ladders are inline in their callers, so neither can
//! be called directly. But the EXPORTED
//! `svt_aom_sig_deriv_multi_processes_{default,allintra}` run all three and
//! leave the answer in `pcs->cdef_level` and `pcs->cdef_search_ctrls`, which
//! `shims/cdef_shims.c` reads back. So this drives the REAL C ladders and the
//! REAL C controls table, arrays included.
//!
//! Two call-site rules are part of what is compared, because they are what the
//! encoder executes:
//!   * the ladder's `is_base` is `pcs->temporal_layer_index == 0`;
//!   * the TABLE's `is_base` is `frame_is_boosted(pcs)` = intra-only OR
//!     ARF OR GF update, and its `is_not_highest_layer` is
//!     `!frame_is_leaf(pcs)` = `update_type != LF_UPDATE`
//!     (`enc_mode_config.h:100-116`) — three different notions of "base"
//!     inside one derivation, and getting them crossed is exactly how a
//!     level-7 frame would silently take a level-5 control set.
//!
//! `allow_intrabc` is an INPUT to the port's ladder and an OUTPUT of C's
//! (C derives it from `set_intrabc_level` a few lines earlier), so the test
//! feeds C's reported value back into the port. The intra-BC ladder itself is
//! covered by `c_parity_sig_deriv_multi_processes.rs`, not here.

use svtav1_cref::cdef_search as cref;
use svtav1_cref::cdef_search::{ARR, cdef_in, cdef_out};
use svtav1_encoder::port_enc_mode_config::ResolutionRange;
use svtav1_encoder::port_enc_mode_config::cdef_search::{
    CdefSearchControls, cdef_search_level_allintra, cdef_search_level_default,
    set_cdef_search_controls,
};

/// C `KEY_FRAME` / `INTER_FRAME` / `INTRA_ONLY_FRAME`.
const KEY_FRAME: i32 = 0;
const INTER_FRAME: i32 = 1;
const INTRA_ONLY_FRAME: i32 = 2;
/// C `SvtAv1FrameUpdateType`.
const KF_UPDATE: i32 = 0;
const LF_UPDATE: i32 = 1;
const GF_UPDATE: i32 = 2;
const ARF_UPDATE: i32 = 3;

#[derive(Clone, Copy, Debug)]
struct Case {
    enc_mode: i8,
    fast_decode: u8,
    input_res: ResolutionRange,
    temporal_layer: u8,
    is_highest_layer: bool,
    is_islice: bool,
    sc_class5: u8,
    seq_cdef_level: u8,
    cfg_cdef_level: i32,
    frame_type: i32,
    update_type: i32,
}

impl Default for Case {
    fn default() -> Self {
        Self {
            enc_mode: 6,
            fast_decode: 0,
            input_res: ResolutionRange::R240p,
            temporal_layer: 0,
            is_highest_layer: false,
            is_islice: true,
            sc_class5: 0,
            seq_cdef_level: 1,
            cfg_cdef_level: -1,
            frame_type: KEY_FRAME,
            update_type: KF_UPDATE,
        }
    }
}

/// Every non-CDEF slot is held at a value the surrounding
/// `svt_aom_sig_deriv_multi_processes_*` body dereferences without trapping;
/// `tf_ctrls.hme_me_level` must stay in 0..=4 or C's HME switch falls through.
fn build_input(c: &Case) -> [i32; cdef_in::COUNT] {
    let mut i = [0i32; cdef_in::COUNT];
    i[cdef_in::ENC_MODE] = i32::from(c.enc_mode);
    i[cdef_in::IS_ISLICE] = i32::from(c.is_islice);
    i[cdef_in::TEMPORAL_LAYER] = i32::from(c.temporal_layer);
    i[cdef_in::INPUT_RES] = i32::from(c.input_res.as_u8());
    i[cdef_in::FAST_DECODE] = i32::from(c.fast_decode);
    i[cdef_in::SC_CLASS5] = i32::from(c.sc_class5);
    i[cdef_in::IS_HIGHEST_LAYER] = i32::from(c.is_highest_layer);
    i[cdef_in::TF_HME_LEVEL] = 0;
    i[cdef_in::ENABLE_INTRABC] = 1;
    i[cdef_in::SEQ_CDEF_LEVEL] = i32::from(c.seq_cdef_level);
    i[cdef_in::CFG_CDEF_LEVEL] = c.cfg_cdef_level;
    i[cdef_in::SEQ_ENABLE_RESTORATION] = 1;
    i[cdef_in::INIT_LUMA_W] = 640;
    i[cdef_in::INIT_LUMA_H] = 480;
    i[cdef_in::ENCODER_BIT_DEPTH] = 8;
    i[cdef_in::CFG_HBD_MDS] = -1;
    i[cdef_in::HBD_MODE_DECISION] = -1;
    i[cdef_in::FRAME_TYPE] = c.frame_type;
    i[cdef_in::UPDATE_TYPE] = c.update_type;
    i
}

/// Rebuild the port's controls from C's own level, so a controls mismatch is
/// never blamed on the ladder (and vice versa).
fn port_ctrls(c: &Case, level: u8) -> CdefSearchControls {
    // `frame_is_boosted` = `frame_is_kf_gf_arf` = intra-only OR ARF OR GF.
    let intra_only = c.frame_type == KEY_FRAME || c.frame_type == INTRA_ONLY_FRAME;
    let is_base = intra_only || c.update_type == ARF_UPDATE || c.update_type == GF_UPDATE;
    // `!frame_is_leaf` = update_type != LF_UPDATE.
    let is_not_highest_layer = c.update_type != LF_UPDATE;
    set_cdef_search_controls(level, is_base, is_not_highest_layer)
        .unwrap_or_else(|| panic!("port refused level {level} for {c:?}"))
}

fn compare(c: &Case, out: &[i64; cdef_out::COUNT], port: &CdefSearchControls, arm: &str) {
    let g = |s: usize| out[s];
    assert_eq!(
        g(cdef_out::ENABLED),
        i64::from(port.enabled),
        "{arm} enabled {c:?}"
    );
    assert_eq!(
        g(cdef_out::FIRST_NUM),
        i64::from(port.first_pass_fs_num),
        "{arm} first_pass_fs_num {c:?}"
    );
    assert_eq!(
        g(cdef_out::SECOND_NUM),
        i64::from(port.default_second_pass_fs_num),
        "{arm} default_second_pass_fs_num {c:?}"
    );
    assert_eq!(
        g(cdef_out::USE_REF_FS),
        i64::from(port.use_reference_cdef_fs),
        "{arm} use_reference_cdef_fs {c:?}"
    );
    assert_eq!(
        g(cdef_out::SUBSAMPLING),
        i64::from(port.subsampling_factor),
        "{arm} subsampling_factor {c:?}"
    );
    assert_eq!(
        g(cdef_out::BEST_REF_FS),
        i64::from(port.search_best_ref_fs),
        "{arm} search_best_ref_fs {c:?}"
    );
    assert_eq!(
        g(cdef_out::SKIP_TH),
        i64::from(port.skip_th),
        "{arm} skip_th {c:?}"
    );
    assert_eq!(
        g(cdef_out::UV_FROM_Y),
        i64::from(port.uv_from_y),
        "{arm} uv_from_y {c:?}"
    );
    assert_eq!(
        g(cdef_out::USE_QP_STRENGTH),
        i64::from(port.use_qp_strength),
        "{arm} use_qp_strength {c:?}"
    );
    // The four candidate arrays, ALL 64 entries each — C's untouched slots
    // are zero on a freshly allocated control set and the port's Default is
    // too, so a partial write on either side shows up.
    for k in 0..ARR {
        assert_eq!(
            g(cdef_out::FIRST_FS + k),
            i64::from(port.default_first_pass_fs[k]),
            "{arm} default_first_pass_fs[{k}] {c:?}"
        );
        assert_eq!(
            g(cdef_out::SECOND_FS + k),
            i64::from(port.default_second_pass_fs[k]),
            "{arm} default_second_pass_fs[{k}] {c:?}"
        );
        assert_eq!(
            g(cdef_out::FIRST_FS_UV + k),
            i64::from(port.default_first_pass_fs_uv[k]),
            "{arm} default_first_pass_fs_uv[{k}] {c:?}"
        );
        assert_eq!(
            g(cdef_out::SECOND_FS_UV + k),
            i64::from(port.default_second_pass_fs_uv[k]),
            "{arm} default_second_pass_fs_uv[{k}] {c:?}"
        );
    }
}

fn cases() -> Vec<Case> {
    let mut v = Vec::new();
    let resolutions = [
        ResolutionRange::R240p,
        ResolutionRange::R360p,
        ResolutionRange::R1080p,
        ResolutionRange::R8k,
    ];
    for enc_mode in -1i8..=13 {
        for fast_decode in [0u8, 1, 2] {
            for &input_res in &resolutions {
                for (frame_type, update_type) in [
                    (KEY_FRAME, KF_UPDATE),
                    (INTER_FRAME, LF_UPDATE),
                    (INTER_FRAME, GF_UPDATE),
                    (INTER_FRAME, ARF_UPDATE),
                    (INTRA_ONLY_FRAME, LF_UPDATE),
                ] {
                    for temporal_layer in [0u8, 1] {
                        v.push(Case {
                            enc_mode,
                            fast_decode,
                            input_res,
                            temporal_layer,
                            is_highest_layer: temporal_layer != 0,
                            is_islice: frame_type != INTER_FRAME,
                            frame_type,
                            update_type,
                            ..Case::default()
                        });
                    }
                }
            }
        }
    }
    // Screen content (drives allow_intrabc, hence the ladders' level-0 arm),
    // the sequence-level kill switch, and every config-forced level.
    for enc_mode in [-1i8, 0, 3, 6, 9, 13] {
        v.push(Case {
            enc_mode,
            sc_class5: 1,
            ..Case::default()
        });
        v.push(Case {
            enc_mode,
            sc_class5: 1,
            is_islice: false,
            frame_type: INTER_FRAME,
            update_type: LF_UPDATE,
            ..Case::default()
        });
        v.push(Case {
            enc_mode,
            seq_cdef_level: 0,
            ..Case::default()
        });
        for cfg in 0i32..=10 {
            v.push(Case {
                enc_mode,
                cfg_cdef_level: cfg,
                ..Case::default()
            });
            v.push(Case {
                enc_mode,
                cfg_cdef_level: cfg,
                frame_type: INTER_FRAME,
                update_type: LF_UPDATE,
                is_islice: false,
                temporal_layer: 1,
                is_highest_layer: true,
                ..Case::default()
            });
        }
    }
    v
}

#[test]
fn cdef_search_level_and_controls_match_c_video_arm() {
    for c in cases() {
        let out = cref::cdef_search_ctrls_default(&build_input(&c));
        let allow_intrabc = out[cdef_out::ALLOW_INTRABC] != 0;
        let level = cdef_search_level_default(
            c.enc_mode,
            c.temporal_layer == 0,
            c.seq_cdef_level,
            allow_intrabc,
            c.cfg_cdef_level,
        );
        assert_eq!(
            out[cdef_out::LEVEL],
            i64::from(level),
            "video cdef_level {c:?}"
        );
        compare(&c, &out, &port_ctrls(&c, level), "video");
    }
}

#[test]
fn cdef_search_level_and_controls_match_c_allintra_arm() {
    for c in cases() {
        let out = cref::cdef_search_ctrls_allintra(&build_input(&c));
        let allow_intrabc = out[cdef_out::ALLOW_INTRABC] != 0;
        let level = cdef_search_level_allintra(
            c.enc_mode,
            c.fast_decode,
            c.input_res,
            c.seq_cdef_level,
            allow_intrabc,
            c.cfg_cdef_level,
        );
        assert_eq!(
            out[cdef_out::LEVEL],
            i64::from(level),
            "allintra cdef_level {c:?}"
        );
        compare(&c, &out, &port_ctrls(&c, level), "allintra");
    }
}

/// ANTI-VACUITY: the sweep must actually reach every level C can assign, and
/// both `use_qp_strength` states. A green run over cases that all land on one
/// level would prove nothing (`WORKING-ON-THIS.md` §5).
#[test]
fn the_sweep_reaches_every_level() {
    let mut seen = [false; 11];
    let mut qp_on = false;
    let mut qp_off = false;
    for c in cases() {
        for out in [
            cref::cdef_search_ctrls_default(&build_input(&c)),
            cref::cdef_search_ctrls_allintra(&build_input(&c)),
        ] {
            let lvl = out[cdef_out::LEVEL] as usize;
            assert!(lvl <= 10, "C produced level {lvl}");
            seen[lvl] = true;
            if out[cdef_out::USE_QP_STRENGTH] != 0 {
                qp_on = true;
            } else {
                qp_off = true;
            }
        }
    }
    for (lvl, hit) in seen.iter().enumerate() {
        assert!(*hit, "no case reached cdef_search_level {lvl}");
    }
    assert!(
        qp_on && qp_off,
        "one use_qp_strength state was never reached"
    );
}
