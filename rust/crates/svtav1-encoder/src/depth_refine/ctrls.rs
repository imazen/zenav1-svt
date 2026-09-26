use super::*;

impl DrCtrls {
    /// C md_config_process.c: lossless overrides the preset ladder with
    /// level 0 (NO_RESTRICTION). The candidate geometry is capped separately.
    pub(crate) fn lossless(disallow_4x4: bool) -> Self {
        Self::for_level(0, disallow_4x4)
    }

    /// C allintra depth-refinement level derivation
    /// (enc_mode_config.c:10067-10090). The level is keyed on `sc_class5`
    /// (screen-content class 5) AND the preset — NOT the preset alone. This is
    /// a clean switch: the r0-modulation (the non-allintra :9350 block) and
    /// `coeff_lvl_modulation` are absent/dead on the allintra I-slice path.
    ///
    /// ```text
    /// sc_class5:  M0/M1 -> 1, M2 -> 5, M3/M4 -> 6, M5 -> 9, M6+ -> 10 (PRED_PART_ONLY)
    /// !sc_class5: M0..M4 -> 6, M5 -> 9, M6+ -> 10
    /// ```
    /// Verified against the instrumented C `depth_refinement_ctrls.mode`/thresholds:
    /// `graph` (sc_class5) reports level 1/1/5/6/6/9 at p0..p5, `codec_wiki`
    /// (!sc_class5) reports 6/6/6/6/6/9 — the port previously used the
    /// !sc_class5 row for every image, over-pruning the depth descent on
    /// screen content at M0-M2 (e1 15 instead of 200/30).
    pub fn for_preset_sc(preset: i8, sc_class5: bool) -> Self {
        let level: u8 = if sc_class5 {
            match preset {
                -1..=1 => 1,
                2 => 5,
                3 | 4 => 6,
                5 => 9,
                _ => 10,
            }
        } else {
            match preset {
                -1 => 3,
                0..=4 => 6,
                5 => 9,
                _ => 10,
            }
        };
        Self::for_level(
            level,
            crate::part_arm::disallow_4x4(crate::sc_detect::ScArm::Allintra, preset),
        )
    }

    /// Pre-fix entry: the !sc_class5 row (level 6 at M0-M4, 9 at M5, 10 at M6+).
    /// A test convenience for the non-screen behaviour; the pipeline calls
    /// [`Self::for_preset_sc`].
    #[cfg(test)]
    pub fn for_preset(preset: i8) -> Self {
        Self::for_preset_sc(preset, false)
    }

    /// The VIDEO arm of the depth-refinement ladder
    /// (`enc_mode_config.c:9350-9396`), plus the `depths_qp_based_th_scaling`
    /// pre-scale C applies to the CHILD thresholds.
    ///
    /// `coeff_lvl` is `pcs->coeff_lvl` (`derive_inter_coeff_level`,
    /// md_config_process.c:650), which the ladder's `<= M3` / `<= M6` / `== M7`
    /// branches split on. On a video-mode I-slice C leaves it `INVALID_LVL`
    /// (:898), which compares as NORMAL in every equality test — pass
    /// `CoeffLvl::Normal` there, the same value
    /// `part_arm::VIDEO_ISLICE_COEFF_LVL` records for the NSQ ladders.
    ///
    /// The r0 modulation at `:9397-9405` is skipped for the same reason
    /// `part_arm::nsq_search_level` skips it: `ppcs->r0_gen` follows
    /// `tpl_ctrls.enable`, and `get_tpl` returns 0 for `LOW_DELAY`, the only
    /// multi-frame shape this port produces.
    ///
    /// # The qp pre-scale, and the C asymmetry it exposes
    ///
    /// `set_qp_based_th_scaling_ctrls_default` (`enc_handle.c:3806`) sets
    /// `depths_qp_based_th_scaling = 1` for every preset above `ENC_MR`,
    /// where the allintra twin (`:3844`-`:3874`) leaves it 0 through M6 —
    /// which is why the still path could use the thresholds RAW.
    ///
    /// C scales them in two places, and only ONE of the two keeps the result:
    ///
    /// - `is_child_to_current_deviation_small` (`enc_dec_process.c:1717-1735`)
    ///   scales `e1`/`e2` into locals and then USES those locals.
    /// - `is_parent_to_current_deviation_small` (`:1637-1659`) scales `s1`/`s2`
    ///   into locals and then OVERWRITES both with
    ///   `ctx->depth_refinement_ctrls.s1_parent_to_current_th + th_offset` —
    ///   the UNSCALED control field. The scaled value survives only as the
    ///   sentinel test. So the parent thresholds are effectively never scaled.
    ///
    /// That asymmetry is reproduced here rather than tidied: a C bug is still
    /// the oracle. Only `e1_th` / `e2_th` are pre-scaled, and the `i64::MIN`
    /// sentinel is preserved through the scale exactly as C's
    /// `(uint8_t)~0 -> MIN_SIGNED_VALUE` mapping does.
    pub fn for_arm(
        arm: crate::sc_detect::ScArm,
        preset: i8,
        sc_class5: bool,
        cli_qp: u32,
        coeff_lvl: crate::quant::CoeffLvl,
    ) -> Self {
        match arm {
            crate::sc_detect::ScArm::Allintra => Self::for_preset_sc(preset, sc_class5),
            crate::sc_detect::ScArm::Video { is_islice } => {
                // C `derive_inter_coeff_level` runs only on non-I-slices
                // (md_config_process.c:898-903): a video I-slice keeps
                // `INVALID_LVL`, which the ladder's equality tests treat as
                // NORMAL — not the caller's (intra-derived) level.
                let coeff_lvl = if is_islice {
                    crate::quant::CoeffLvl::Normal
                } else {
                    coeff_lvl
                };
                let level: u8 = if sc_class5 {
                    match preset {
                        -1..=2 => 0,
                        3 => u8::from(!is_islice),
                        4 => 1,
                        5 => {
                            if is_islice {
                                1
                            } else {
                                4
                            }
                        }
                        6 => 4,
                        7 | 8 => 6,
                        9 => 7,
                        _ => 9,
                    }
                } else {
                    // enc_mode_config.c:9368-9390 — the !sc_class5 row splits
                    // on `pcs->coeff_lvl` at every rung from M1 through M7.
                    use crate::quant::CoeffLvl;
                    let low = matches!(coeff_lvl, CoeffLvl::VLow | CoeffLvl::Low);
                    match preset {
                        -1 | 0 => 0,
                        1..=3 => {
                            if low {
                                2
                            } else {
                                3
                            }
                        }
                        4..=6 => {
                            if low {
                                5
                            } else if coeff_lvl == CoeffLvl::High {
                                7
                            } else {
                                6
                            }
                        }
                        7 => {
                            if low {
                                6
                            } else if coeff_lvl == CoeffLvl::High {
                                10
                            } else {
                                8
                            }
                        }
                        _ => 10,
                    }
                };
                let mut c = Self::for_level(level, crate::part_arm::disallow_4x4(arm, preset));
                // `q_weight` is 1 in every level the table enables, and 0 at
                // level 0 (PD0_DEPTH_NO_RESTRICTION assigns nothing else) —
                // where there is no deviation gate to scale anyway.
                if level != 0 {
                    let (qw, qwd) = crate::pd0::qp_th_scaling_factors(cli_qp);
                    let scale = |th: i64| -> i64 {
                        if th == S2E2_ALWAYS {
                            th
                        } else {
                            // C DIVIDE_AND_ROUND(a, b) = (a + b/2) / b.
                            let (qw, qwd) = (i64::from(qw), i64::from(qwd));
                            (th * qw + qwd / 2) / qwd
                        }
                    };
                    c.e1_th = scale(c.e1_th);
                    c.e2_th = scale(c.e2_th);
                }
                c
            }
        }
    }

    /// Build the ctrls for a `set_block_based_depth_refinement_controls` level
    /// (enc_mode_config.c:6816). `disallow_4x4` is preset-AND-ARM based
    /// (`crate::part_arm::disallow_4x4`: allintra `> M3`, video `> M2`) and
    /// independent of the level, so the caller resolves it and passes it in.
    pub(super) fn for_level(level: u8, disallow_4x4: bool) -> Self {
        // C `set_block_based_depth_refinement_controls` case 10 (:6986) is the
        // only row that sets `mode = PD0_DEPTH_PRED_PART_ONLY`.
        let pred_depth_only = level == 10;
        match level {
            // case 1: sc_class5 M0/M1. s2/e2 = literal 0 (NOT the sentinel).
            1 => DrCtrls {
                adaptive: true,
                no_restriction: false,
                s1_th: 200,
                e1_th: 200,
                s2_th: 0,
                e2_th: 0,
                use_ref_info: false,
                parent_max_cost_mult: 10,
                band_mod: false,
                max_cost_multiplier: 0,
                max_band_cnt: 1,
                decrement_per_band: [0; 4],
                lower_split_th: 0,
                split_rate_th: 0,
                limit_to_pd0: 0,
                unavail_mode: 2,
                disallow_4x4,
                coeff_lvl_mod: true,
                pred_depth_only,
            },
            // case 5: sc_class5 M2. s2/e2 = sentinel (always passes).
            5 => DrCtrls {
                adaptive: true,
                no_restriction: false,
                s1_th: 30,
                e1_th: 30,
                s2_th: S2E2_ALWAYS,
                e2_th: S2E2_ALWAYS,
                use_ref_info: false,
                parent_max_cost_mult: 10,
                band_mod: false,
                max_cost_multiplier: 0,
                max_band_cnt: 1,
                decrement_per_band: [0; 4],
                lower_split_th: 10,
                split_rate_th: 10,
                limit_to_pd0: 2,
                unavail_mode: 2,
                disallow_4x4,
                coeff_lvl_mod: true,
                pred_depth_only,
            },
            // case 6: M0-M4 (!sc_class5) and sc_class5 M3/M4.
            6 => DrCtrls {
                adaptive: true,
                no_restriction: false,
                s1_th: 15,
                e1_th: 15,
                s2_th: S2E2_ALWAYS,
                e2_th: S2E2_ALWAYS,
                use_ref_info: false,
                parent_max_cost_mult: 10,
                band_mod: false,
                max_cost_multiplier: 0,
                max_band_cnt: 1,
                decrement_per_band: [0; 4],
                lower_split_th: 20,
                split_rate_th: 10,
                limit_to_pd0: 1,
                unavail_mode: 2,
                disallow_4x4,
                coeff_lvl_mod: true,
                pred_depth_only,
            },
            // case 9: M5.
            9 => DrCtrls {
                adaptive: true,
                no_restriction: false,
                s1_th: 10,
                e1_th: 10,
                s2_th: S2E2_ALWAYS,
                e2_th: S2E2_ALWAYS,
                use_ref_info: true,
                parent_max_cost_mult: 0,
                band_mod: true,
                max_cost_multiplier: 400,
                max_band_cnt: 4,
                decrement_per_band: [i64::MAX, i64::MAX, 10, 5],
                lower_split_th: 100,
                split_rate_th: 5,
                limit_to_pd0: 1,
                unavail_mode: 0,
                disallow_4x4,
                coeff_lvl_mod: true,
                pred_depth_only,
            },
            // case 0: PD0_DEPTH_NO_RESTRICTION — every field but `mode` is
            // left at whatever the context held, and none of them is read
            // because the narrowing block is skipped. The values below are
            // therefore inert placeholders, not transcriptions.
            0 => DrCtrls {
                adaptive: true,
                no_restriction: true,
                s1_th: 0,
                e1_th: 0,
                s2_th: S2E2_ALWAYS,
                e2_th: S2E2_ALWAYS,
                use_ref_info: false,
                parent_max_cost_mult: 0,
                band_mod: false,
                max_cost_multiplier: 0,
                max_band_cnt: 1,
                decrement_per_band: [0; 4],
                lower_split_th: 0,
                split_rate_th: 0,
                limit_to_pd0: 0,
                unavail_mode: 2,
                disallow_4x4,
                coeff_lvl_mod: false,
                pred_depth_only,
            },
            // case 2 / case 3 / case 4: video-only, identical to case 1 except
            // for the s1/e1 threshold and the two split thresholds.
            2 | 3 | 4 => DrCtrls {
                adaptive: true,
                no_restriction: false,
                s1_th: match level {
                    2 => 90,
                    3 => 60,
                    _ => 30,
                },
                e1_th: match level {
                    2 => 90,
                    3 => 60,
                    _ => 30,
                },
                s2_th: 0,
                e2_th: 0,
                use_ref_info: false,
                parent_max_cost_mult: 10,
                band_mod: false,
                max_cost_multiplier: 0,
                max_band_cnt: 1,
                decrement_per_band: [0; 4],
                lower_split_th: 10,
                split_rate_th: 10,
                limit_to_pd0: 0,
                unavail_mode: 2,
                disallow_4x4,
                coeff_lvl_mod: true,
                pred_depth_only,
            },
            // case 7 / case 8: video-only, the cost-band-modulated rows below
            // case 9. Case 8 additionally drops `pd0_unavail_mode_depth` to 0.
            7 | 8 => DrCtrls {
                adaptive: true,
                no_restriction: false,
                s1_th: if level == 7 { 15 } else { 10 },
                e1_th: if level == 7 { 15 } else { 10 },
                s2_th: S2E2_ALWAYS,
                e2_th: S2E2_ALWAYS,
                use_ref_info: true,
                parent_max_cost_mult: 0,
                band_mod: true,
                max_cost_multiplier: 400,
                max_band_cnt: 4,
                decrement_per_band: [i64::MAX, i64::MAX, 10, 5],
                lower_split_th: if level == 7 { 20 } else { 25 },
                split_rate_th: 5,
                limit_to_pd0: 1,
                unavail_mode: if level == 7 { 2 } else { 0 },
                disallow_4x4,
                coeff_lvl_mod: true,
                pred_depth_only,
            },
            // case 10 (M6+): PRED_PART_ONLY — s = e = 0 everywhere.
            _ => DrCtrls {
                adaptive: false,
                no_restriction: false,
                s1_th: 0,
                e1_th: 0,
                s2_th: S2E2_ALWAYS,
                e2_th: S2E2_ALWAYS,
                use_ref_info: false,
                parent_max_cost_mult: 0,
                band_mod: false,
                max_cost_multiplier: 0,
                max_band_cnt: 1,
                decrement_per_band: [0; 4],
                lower_split_th: 0,
                split_rate_th: 0,
                limit_to_pd0: 0,
                unavail_mode: 0,
                disallow_4x4,
                coeff_lvl_mod: false,
                pred_depth_only,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Refined depth scan (C MdScan marks after perform_pred_depth_refinement)
// ---------------------------------------------------------------------------

/// One square node of the refined scan.
#[derive(Debug, Clone)]
pub struct RefScan {
    pub sq: usize,
    /// C `mds->tot_shapes == 1`: evaluate PART_N at this node.
    pub test_this: bool,
    /// C `mds->split_flag`: recurse into the children.
    pub split_flag: bool,
    pub children: Option<Box<[RefScan; 4]>>,
}

impl RefScan {
    pub(super) fn leaf(sq: usize) -> Self {
        RefScan {
            sq,
            test_this: false,
            split_flag: false,
            children: None,
        }
    }

    /// C `set_child_to_be_tested` (enc_dec_process.c:1522): mark the
    /// child depth for evaluation (`disallow_4x4` blocks 8x8 -> 4x4,
    /// `disallow_8x8` blocks 16x16 -> 8x8).
    pub(super) fn set_children_tested(
        &mut self,
        e_depth: i32,
        disallow_4x4: bool,
        disallow_8x8: bool,
    ) {
        // 4x4 never has children.
        if self.sq <= 4 || (self.sq == 8 && disallow_4x4) || (self.sq == 16 && disallow_8x8) {
            return;
        }
        self.split_flag = true;
        let half = self.sq / 2;
        let mut ch: [RefScan; 4] = [
            RefScan::leaf(half),
            RefScan::leaf(half),
            RefScan::leaf(half),
            RefScan::leaf(half),
        ];
        for c in ch.iter_mut() {
            c.test_this = true;
            if e_depth > 1 {
                c.set_children_tested(e_depth - 1, disallow_4x4, disallow_8x8);
            }
        }
        self.children = Some(Box::new(ch));
    }
}
