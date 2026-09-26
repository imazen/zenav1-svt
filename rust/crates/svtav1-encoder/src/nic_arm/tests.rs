use super::*;

/// The still path against `FunnelCfg::for_preset`'s baked rows.
///
/// ONE row disagrees, and the ladder walk is the correct side:
/// `for_preset`'s `8 =>` and `_ =>` arms spread `m6_tail`, which carries
/// `mds3_class_th = 5` (nic_level 6). The allintra arm picks nic_level 11
/// at M8 and above, and `set_nic_controls` case 11 sets `mds3_class_th =
/// 0`. The two differ only through `post_mds2_nic_pruning`'s I-slice form
/// `MAX(25, base * i_mds3_class_th_mult)` (`product_coding_loop.c:7978`) —
/// 250 vs 25 — which is reachable only on a MULTI-CLASS leaf, i.e. one
/// with a palette candidate. That is why it went unnoticed: allintra
/// screen-content detection is off above M7 (`derive_allintra_sc`), so a
/// stock M8+ still frame has `palette_level = 0` and a single class. It is
/// NOT provably dead under tune-IQ, which forces the detector on at every
/// preset, so this is stamped rather than excused, and the still envelope
/// is re-measured rather than assumed.
#[test]
fn allintra_flattening_matches_the_ladder() {
    for preset in 0i8..=13 {
        let baked = FunnelCfg::for_preset(preset);
        let mut walked = baked;
        let eff = crate::rate_arm::eff_enc_mode(ScArm::Allintra, preset);
        apply(
            &mut walked,
            ScArm::Allintra,
            eff,
            true,
            crate::reference::SvtReference::Hybrid3115,
        );
        // A NAMED list rather than a tuple: it names the offender on a
        // failure, and a tuple wide enough for every NIC field is past
        // the arity `Debug`/`PartialEq` are implemented at.
        let fields = |c: &FunnelCfg| {
            alloc::vec![
                ("nic_num.0", c.nic_num.0),
                ("nic_num.1", c.nic_num.1),
                ("nic_num.2", c.nic_num.2),
                ("mds1_cand_base_th", c.mds1_cand_base_th),
                ("mds1_cand_base_th_inter", c.mds1_cand_base_th_inter),
                ("mds1_rank_factor", c.mds1_rank_factor),
                ("mds1_class_th", c.mds1_class_th),
                ("mds1_band_cnt", u64::from(c.mds1_band_cnt)),
                ("mds2_cand_base_th", c.mds2_cand_base_th),
                ("mds2_rank_factor", c.mds2_rank_factor),
                ("mds2_rel_dev_th", c.mds2_rel_dev_th),
                ("mds2_class_th", c.mds2_class_th),
                ("mds2_band_cnt", u64::from(c.mds2_band_cnt)),
                ("mds3_cand_base_th", c.mds3_cand_base_th),
                ("mds3_band_cnt", u64::from(c.mds3_band_cnt)),
                ("i_mds3_class_th_mult", c.i_mds3_class_th_mult),
                ("enable_skipping_mds1", u64::from(c.enable_skipping_mds1)),
                (
                    "merge_inter_cands_mult",
                    u64::from(c.merge_inter_cands_mult),
                ),
                ("nic_txt_qp_scaling", u64::from(c.nic_txt_qp_scaling)),
            ]
        };
        assert_eq!(
            fields(&baked),
            fields(&walked),
            "allintra nic ladder vs FunnelCfg::for_preset at M{preset}"
        );
        let want_class_th = if preset >= 8 { 0 } else { baked.mds3_class_th };
        assert_eq!(
            want_class_th, walked.mds3_class_th,
            "allintra mds3_class_th at M{preset}"
        );
    }
}

/// TIER 1: every [`NicRow`] field at every level against the real
/// `svt_aom_set_nic_controls`, run by the cref shim on a zeroed context
/// (a field C leaves unassigned reads as 0 there, as it does here). The
/// scaling row is compared as the three stage nums C stamps, not as the
/// level index, so a wrong `NICS_SCAL_NUM` row would fail too.
#[test]
fn nic_ctrls_matches_the_real_c_at_every_level() {
    let mut skipping_levels = 0usize;
    for level in 0u8..=11 {
        let (_sc, c) = svtav1_cref::mode_decision::set_nic_controls(level);
        let r = nic_ctrls(level);
        // A NAMED list, as in the ladder test: a 16-tuple is past the
        // arity `Debug`/`PartialEq` are implemented at.
        let want: alloc::vec::Vec<(&str, u64)> = alloc::vec![
            ("nic_num.0", u64::from(c.stage1_scaling_num)),
            ("nic_num.1", u64::from(c.stage2_scaling_num)),
            ("nic_num.2", u64::from(c.stage3_scaling_num)),
            ("mds1_cand_base_th", c.mds1_cand_base_th_intra),
            ("mds1_cand_base_th_inter", c.mds1_cand_base_th_inter),
            ("mds1_class_th", c.mds1_class_th),
            ("mds1_band_cnt", u64::from(c.mds1_band_cnt)),
            ("mds2_class_th", c.mds2_class_th),
            ("mds2_band_cnt", u64::from(c.mds2_band_cnt)),
            ("mds1_rank_factor", u64::from(c.mds1_cand_th_rank_factor)),
            ("mds2_cand_base_th", c.mds2_cand_base_th),
            ("mds2_rank_factor", u64::from(c.mds2_cand_th_rank_factor)),
            ("mds2_rel_dev_th", u64::from(c.mds2_relative_dev_th)),
            ("mds3_cand_base_th", c.mds3_cand_base_th),
            ("mds3_class_th", c.mds3_class_th),
            ("mds3_band_cnt", u64::from(c.mds3_band_cnt)),
            ("i_mds3_class_th_mult", u64::from(c.i_mds3_class_th_mult)),
            ("enable_skipping_mds1", u64::from(c.enable_skipping_mds1)),
            (
                "merge_inter_cands_mult",
                u64::from(c.merge_inter_cands_mult),
            ),
        ];
        let got: alloc::vec::Vec<(&str, u64)> = alloc::vec![
            ("nic_num.0", r.nic_num.0),
            ("nic_num.1", r.nic_num.1),
            ("nic_num.2", r.nic_num.2),
            ("mds1_cand_base_th", r.mds1_cand_base_th),
            ("mds1_cand_base_th_inter", r.mds1_cand_base_th_inter),
            ("mds1_class_th", r.mds1_class_th),
            ("mds1_band_cnt", u64::from(r.mds1_band_cnt)),
            ("mds2_class_th", r.mds2_class_th),
            ("mds2_band_cnt", u64::from(r.mds2_band_cnt)),
            ("mds1_rank_factor", r.mds1_rank_factor),
            ("mds2_cand_base_th", r.mds2_cand_base_th),
            ("mds2_rank_factor", r.mds2_rank_factor),
            ("mds2_rel_dev_th", r.mds2_rel_dev_th),
            ("mds3_cand_base_th", r.mds3_cand_base_th),
            ("mds3_class_th", r.mds3_class_th),
            ("mds3_band_cnt", u64::from(r.mds3_band_cnt)),
            ("i_mds3_class_th_mult", r.i_mds3_class_th_mult),
            ("enable_skipping_mds1", u64::from(r.enable_skipping_mds1)),
            (
                "merge_inter_cands_mult",
                u64::from(r.merge_inter_cands_mult),
            ),
        ];
        assert_eq!(
            got, want,
            "nic_ctrls vs svt_aom_set_nic_controls at level {level}"
        );
        skipping_levels += usize::from(c.enable_skipping_mds1);
    }
    // ANTI-VACUITY: the flag must be observable on both sides of the
    // ladder, or agreement could be agreement on a constant.
    assert_eq!(
        skipping_levels, 4,
        "C sets enable_skipping_mds1 at exactly levels 8..=11"
    );
}

/// The M6 key-frame row the inter campaign stands on.
#[test]
fn video_m6_key_frame_tightens_every_nic_stage() {
    let arm = ScArm::Video { is_islice: true };
    assert_eq!(
        nic_level(arm, 6, true, crate::reference::SvtReference::Hybrid3115),
        8
    );
    let r = nic_ctrls(8);
    assert_eq!(r.nic_num, (2, 1, 1));
    assert_eq!(
        (
            r.mds1_cand_base_th,
            r.mds2_cand_base_th,
            r.mds3_cand_base_th
        ),
        (300, 3, 3)
    );
    // The still path at M6 is nic_level 6: four times the stage-1 count
    // and four to five times the candidate thresholds.
    let m6 = FunnelCfg::for_preset(6);
    assert_eq!(m6.nic_num, (6, 6, 6));
    assert_eq!(
        (
            m6.mds1_cand_base_th,
            m6.mds2_cand_base_th,
            m6.mds3_cand_base_th
        ),
        (1200, 15, 15)
    );
}

/// `qp_based_th_scaling_ctrls.{nic_max, nic_pruning, txt}` — the one 0 in
/// the whole set is `set_qp_based_th_scaling_ctrls_default`'s MR row
/// (enc_handle.c:3789-3802). `nic_counts`'s qp scale and the funnel's
/// pruning/txt scales all read this bit, so a stale `true` at video p-1
/// caps MDS3 at `scal * qp/63` (~13 of 20 at qp 40) and drops the winner.
#[test]
fn video_mr_disables_the_qp_scalers() {
    let mut saw_on = 0usize;
    for preset in -1..=13i8 {
        let mut cfg = FunnelCfg::for_preset(preset.max(0));
        let video = crate::rate_arm::eff_enc_mode(ScArm::Video { is_islice: true }, preset);
        apply(
            &mut cfg,
            ScArm::Video { is_islice: true },
            video,
            true,
            crate::reference::SvtReference::Mainline420,
        );
        assert_eq!(
            cfg.nic_txt_qp_scaling,
            preset > -1,
            "nic/txt qp scaling at video preset {preset}"
        );
        saw_on += usize::from(cfg.nic_txt_qp_scaling);
        // The still arm is on at every preset, including -1.
        let mut still = FunnelCfg::for_preset(preset.max(0));
        let allintra = crate::rate_arm::eff_enc_mode(ScArm::Allintra, preset);
        apply(
            &mut still,
            ScArm::Allintra,
            allintra,
            true,
            crate::reference::SvtReference::Mainline420,
        );
        assert!(still.nic_txt_qp_scaling, "allintra p{preset}");
    }
    assert_eq!(saw_on, 14, "every video preset above MR must stay on");
}
