use super::*;
use crate::port_enc_mode_config::ResolutionRange;
use crate::port_enc_mode_config::cdef_search::{
    CONFIG_DEFAULT, PF_GI, cdef_search_level_allintra, set_cdef_search_controls,
};

/// The candidate table the port carried BEFORE `set_cdef_search_controls`
/// was ported: the allintra ladder's outcome flattened per preset, which
/// is what every byte of the 280/280 still envelope was produced with.
/// Kept as the regression oracle for
/// [`allintra_flattening_matches_the_ladder`] — deleting it would delete
/// the only independent statement of what the still path used to do.
fn allintra_cfg_for_preset_flattened(preset: i8) -> CdefSearchCfg {
    let (first, extra): (&[usize], &[i32]) = match preset {
        0 => (&[0, 1, 2, 4, 5, 6, 8, 9, 10, 12, 13, 14], &[1, 2, 3]),
        1..=3 => (&[0, 4, 8, 12, 15], &[1, 2, 3]),
        4 | 5 => (&[0, 7, 15], &[2]),
        _ => (&[0, 15], &[2]),
    };
    let mut fs: Vec<i32> = first.iter().map(|&i| i32::from(PF_GI[i])).collect();
    let first_pass_num = fs.len();
    for &i in first {
        for &d in extra {
            fs.push(i32::from(PF_GI[i]) + d);
        }
    }
    let chroma_search = (0..fs.len()).map(|i| i < first_pass_num).collect();
    CdefSearchCfg {
        fs,
        first_pass_num,
        chroma_search,
        // This helper reproduces the SEARCH-candidate flattening only;
        // the recon-controls bias is asserted separately.
        zero_fs_cost_bias: 0,
        subsampling: if preset >= 6 { 4 } else { 1 },
    }
}

/// NO-STILL-REGRESSION, at the unit level: routing the still path through
/// the real C ladder + controls table reproduces the flattened per-preset
/// table entry for entry, at every preset the still search can reach
/// (0..=6 — 7+ is level 10, the qp fast path). This is what makes the
/// pipeline rewrite byte-neutral for `is_single_frame`; the byte gates
/// then confirm it end to end.
#[test]
fn allintra_flattening_matches_the_ladder() {
    for preset in 0..=6i8 {
        let level = cdef_search_level_allintra(
            preset as i8,
            0,
            ResolutionRange::R240p,
            1,
            false,
            CONFIG_DEFAULT,
        );
        let ctrls = set_cdef_search_controls(
            level,
            true,
            true,
            crate::reference::SvtReference::Hybrid3115,
        )
        .unwrap();
        assert!(!ctrls.use_qp_strength, "preset {preset} level {level}");
        let got = cdef_search_cfg_from_ctrls(&ctrls, 0);
        let want = allintra_cfg_for_preset_flattened(preset);
        assert_eq!(
            got.fs, want.fs,
            "preset {preset} (level {level}) candidates"
        );
        assert_eq!(got.first_pass_num, want.first_pass_num, "preset {preset}");
        assert_eq!(got.subsampling, want.subsampling, "preset {preset}");
    }
    // 7+ takes the qp fast path, which is the other half of the old
    // `allintra_preset_uses_cdef_search` predicate.
    for preset in 7..=13i8 {
        let level = cdef_search_level_allintra(
            preset as i8,
            0,
            ResolutionRange::R240p,
            1,
            false,
            CONFIG_DEFAULT,
        );
        assert_eq!(level, 10, "preset {preset}");
        assert!(
            set_cdef_search_controls(
                level,
                true,
                true,
                crate::reference::SvtReference::Hybrid3115
            )
            .unwrap()
            .use_qp_strength
        );
        assert!(!allintra_preset_uses_cdef_search(preset));
    }
}

/// [`cdef_search_cfg_from_ctrls`] drops four control fields. This pins
/// the claim that they are inert on a KEY frame at every level a key
/// frame reaches through either ladder, so the projection loses nothing
/// TODAY — and fails loudly if an inter arm ever routes a level here
/// where they are not.
#[test]
fn key_frame_ref_fields_are_inert() {
    // Levels a KEY frame can reach: allintra 1/2/3/5/7/10, video
    // 1/2/5/7. Levels 8/9 are RTC-only and are NOT asserted inert.
    for level in [1u8, 2, 3, 5, 7, 10] {
        let c = set_cdef_search_controls(
            level,
            true,
            true,
            crate::reference::SvtReference::Hybrid3115,
        )
        .unwrap();
        assert_eq!(c.use_reference_cdef_fs, 0, "level {level}");
        assert_eq!(c.search_best_ref_fs, 0, "level {level}");
        assert_eq!(c.skip_th, 0, "level {level}");
        assert!(!c.uv_from_y, "level {level}");
    }
}

/// C-verified anchors (values cross-checked bit-exact against the C
/// float evaluation for ALL qindexes by
/// tests/c_parity_cdef_pick.rs; these pin representative points):
/// q(255): ac=1828 -> y = 63 (pri 15, sec field 3), uv = 3 (uv_f1's
/// quadratic goes negative and clamps to 0 — C's own fit);
/// q(128): ac=176 -> y = 9, uv = 8; q(30): ac=37 -> all zero.
#[test]
fn strength_formula_anchors() {
    let p255 = pick_cdef_params_key_frame(255, 8, false);
    assert_eq!(
        (p255.damping, p255.y_strength, p255.uv_strength),
        (6, 63, 3)
    );
    let p128 = pick_cdef_params_key_frame(128, 8, false);
    assert_eq!((p128.damping, p128.y_strength, p128.uv_strength), (5, 9, 8));
    // Very low q: everything zero (CDEF off near-lossless).
    let p30 = pick_cdef_params_key_frame(30, 8, false);
    assert_eq!((p30.y_strength, p30.uv_strength), (0, 0));
    assert_eq!(p30.damping, 3);
    assert!(!p30.any(true) && !p30.any(false));
}

/// The full recon-parity matrix qindexes {80,128,172,220,255} must all
/// produce nonzero luma strengths (non-vacuous gate coverage; C-verified
/// values y = 4/9/17/43/63) and legal field ranges everywhere.
#[test]
fn firing_profile_and_ranges() {
    for q in 0..=255u16 {
        let p = pick_cdef_params_key_frame(q as u8, 8, false);
        assert!((3..=6).contains(&p.damping), "damping range at {q}");
        assert!(p.y_strength <= 63 && p.uv_strength <= 63);
    }
    // Zero below the knee (near-lossless protection)...
    assert_eq!(pick_cdef_params_key_frame(50, 8, false).y_strength, 0);
    // ...firing across the entire gate matrix.
    for (q, want_y) in [(80u8, 4u8), (128, 9), (172, 17), (220, 43), (255, 63)] {
        assert_eq!(
            pick_cdef_params_key_frame(q, 8, false).y_strength,
            want_y,
            "luma CDEF strength at qindex {q}"
        );
    }
    assert!(pick_cdef_params_key_frame(80, 8, false).uv_strength != 0);
}

/// bd10 qp-fast-path anchors (task #94). C `svt_pick_cdef_from_qp`
/// (enc_cdef.c:829-830) normalizes the AC step back to the 8-bit scale
/// via `q = ac_quant_qtx(qindex, 0, 10) >> 2` and reuses the SAME intra
/// fit constants — so the bd10 strengths differ from bd8 (the >>2 of
/// AC_QLOOKUP_10 is NOT exactly AC_QLOOKUP_8). These values are the ones
/// the port emits into the frame header at bd10, PROVEN C-correct by the
/// end-to-end FH byte-match in the gradient bd10 op-trace (the entire C
/// frame header — cdef_y/uv_strength, damping, bits — is byte-identical
/// to the port's once this bd10 arm lands; see docs/bd10-port-map.md).
#[test]
fn strength_formula_anchors_bd10() {
    // qindex 160 (cli qp 40): FH-byte-verified (gradient 64x64 q40 p13
    // bd10 op-trace — first divergence moved off FH onto the tile).
    let p160 = pick_cdef_params_key_frame(160, 10, false);
    assert_eq!(
        (p160.damping, p160.y_strength, p160.uv_strength),
        (5, 13, 12)
    );
    // A spread of qindexes across the fit's range, hand-traced from
    // AC_QLOOKUP_10>>2 + the intra fit (bd10 differs from bd8 here).
    let p172 = pick_cdef_params_key_frame(172, 10, false);
    assert_eq!(
        (p172.damping, p172.y_strength, p172.uv_strength),
        (5, 17, 13)
    );
    let p220 = pick_cdef_params_key_frame(220, 10, false);
    assert_eq!(
        (p220.damping, p220.y_strength, p220.uv_strength),
        (6, 43, 7)
    );
    let p255 = pick_cdef_params_key_frame(255, 10, false);
    assert_eq!(
        (p255.damping, p255.y_strength, p255.uv_strength),
        (6, 63, 3)
    );
    // Contrast: bd8 and bd10 genuinely diverge (the whole point of the
    // fix). 16 qindexes differ; the knee shifts because AC_QLOOKUP_10>>2
    // crosses the CDEF-off threshold at a different qindex than
    // AC_QLOOKUP_8. q52: luma strength 4 (bd8) vs 0 (bd10).
    assert_eq!(pick_cdef_params_key_frame(52, 8, false).y_strength, 4);
    assert_eq!(pick_cdef_params_key_frame(52, 10, false).y_strength, 0);
}

/// The finish_cdef_search RD pick pinned against the instrumented C
/// captures (SVT_M6DBG CDEFMSE/CDEFBITS/CDEFPICK, gradient p6 cells,
/// docs/IDENTITY-STATUS.md M6 chunk).
#[test]
fn finish_rd_matches_c_captures() {
    fn row(y: [u64; 4], uv: [u64; 4]) -> MseRow {
        [y.to_vec(), uv.to_vec()]
    }
    // g64 q55 (qindex 220): pick y search-index 2 (strength 2 =
    // pri 0 / sec 2), uv index 0, cdef_bits 0.
    let m55 = [row(
        [885_020, 900_992, 875_920, 892_836],
        [0, 0, 66_585_600, 66_585_600],
    )];
    assert_eq!(finish_cdef_rd(&m55, 4, 220).0, 0);
    assert_eq!(finish_cdef_rd(&m55, 4, 220).1, alloc::vec![2]);
    assert_eq!(finish_cdef_rd(&m55, 4, 220).2, alloc::vec![0]);
    // g64 q40 (qindex 160): pick y index 3 (strength 62 = pri 15 /
    // sec 2).
    let m40 = [row(
        [271_716, 257_812, 260_308, 251_848],
        [0, 0, 66_585_600, 66_585_600],
    )];
    let (b40, l0_40, l1_40, dev40) = finish_cdef_rd(&m40, 4, 160);
    assert_eq!((b40, l0_40, l1_40), (0, alloc::vec![3], alloc::vec![0]));
    // `cdef_dist_dev` (enc_cdef.c:1057): the pick reduces RD cost vs the
    // zero-strength arm, so dev is positive and bounded by 1000.
    assert!((0..1000).contains(&dev40));
    // g128 q20 (qindex 80), 4 filter blocks: pick y index 2
    // (strength 2), uv 0, bits 0 (CDEFPICK y=[2]).
    let uvrow = [0u64, 0, 66_585_600, 66_585_600];
    let m20 = [
        row([51_440, 48_480, 47_460, 49_720], uvrow),
        row([49_580, 46_280, 45_176, 47_436], uvrow),
        row([52_756, 49_508, 48_144, 49_800], uvrow),
        row([52_028, 48_140, 46_548, 48_664], uvrow),
    ];
    let (b20, l0_20, l1_20, dev20) = finish_cdef_rd(&m20, 4, 80);
    assert_eq!((b20, l0_20, l1_20), (0, alloc::vec![2], alloc::vec![0]));
    assert!((0..1000).contains(&dev20));
}

/// Damping steps exactly at the C breakpoints.
#[test]
fn damping_from_qp_breakpoints() {
    assert_eq!(pick_cdef_params_key_frame(0, 8, false).damping, 3);
    assert_eq!(pick_cdef_params_key_frame(63, 8, false).damping, 3);
    assert_eq!(pick_cdef_params_key_frame(64, 8, false).damping, 4);
    assert_eq!(pick_cdef_params_key_frame(127, 8, false).damping, 4);
    assert_eq!(pick_cdef_params_key_frame(128, 8, false).damping, 5);
    assert_eq!(pick_cdef_params_key_frame(191, 8, false).damping, 5);
    assert_eq!(pick_cdef_params_key_frame(192, 8, false).damping, 6);
}
