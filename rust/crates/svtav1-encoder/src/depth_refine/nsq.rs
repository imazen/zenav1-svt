use super::*;

/// `partition_fac_bits[PARTITION_CONTEXTS][..]` — per-row costs from a
/// (possibly chained) frame context's partition CDFs. Row layout matches
/// the writer: `bsl * 4 + (left*2 + above)`; rows 0..3 (8x8) carry 4
/// symbols, 4..15 carry 10 (64-SB frames never touch the 128 rows).
pub(crate) struct PartRates {
    pub(super) rows: [[i32; 10]; 16],
    /// C `partition_vert_alike_fac_bits[ctx][p == PARTITION_SPLIT]` — the
    /// BOTTOM-edge (`!has_rows`) binary alphabet (md_rate_estimation.c:89-97).
    pub(super) vert_alike: [[u32; 2]; 16],
    /// C `partition_horz_alike_fac_bits[ctx][p == PARTITION_SPLIT]` — the
    /// RIGHT-edge (`!has_cols`) binary alphabet (md_rate_estimation.c:110-118).
    pub(super) horz_alike: [[u32; 2]; 16],
}

impl PartRates {
    pub(crate) fn from_fc(fc: &crate::entropy::context::FrameContext) -> Self {
        let mut rows = [[0i32; 10]; 16];
        let mut vert_alike = [[0u32; 2]; 16];
        let mut horz_alike = [[0u32; 2]; 16];
        for (row, out) in rows.iter_mut().enumerate() {
            let nsyms = if row < 4 { 4 } else { 10 };
            crate::quant::syntax_rate_from_cdf(&mut out[..nsyms], &fc.partition_cdf[row]);
            // is_128 = false: SB64 squares are <= 64x64. C builds both the
            // 16x16 and the 128x128 gather per context row; the 128 rows are
            // unreachable here (an SB128 refined path would need the `true`
            // variant, which `partition_alike_costs` already takes).
            vert_alike[row] = crate::entropy::context::partition_alike_costs(
                &fc.partition_cdf[row],
                true, // !has_rows -> vert_alike (bottom edge)
                false,
            );
            horz_alike[row] = crate::entropy::context::partition_alike_costs(
                &fc.partition_cdf[row],
                false, // !has_cols -> horz_alike (right edge)
                false,
            );
        }
        PartRates {
            rows,
            vert_alike,
            horz_alike,
        }
    }

    /// `svt_aom_partition_rate_cost` (rd_cost.c:1834) for in-frame square
    /// blocks (has_rows && has_cols — 64-aligned frames only reach here):
    /// context row from the partition neighbour bytes.
    #[inline]
    pub(crate) fn bits(&self, ctx_row: usize, p: PartitionType) -> u64 {
        debug_assert!(ctx_row < 16);
        self.rows[ctx_row][p as usize] as u64
    }

    /// The full derived rate row — NSQDBG only.
    #[cfg(feature = "std")]
    pub(crate) fn row(&self, ctx_row: usize) -> [i32; 10] {
        self.rows[ctx_row]
    }

    /// The FULL `svt_aom_partition_rate_cost` (rd_cost.c:1834-1867), including
    /// the two frame-boundary arms the port previously never reached:
    ///
    /// * `!has_rows && !has_cols` -> 0 (the node codes no partition symbol);
    /// * `!has_rows && has_cols`  -> `partition_vert_alike_fac_bits[ctx][split]`;
    /// * `has_rows && !has_cols`  -> `partition_horz_alike_fac_bits[ctx][split]`.
    ///
    /// On a 64-aligned frame every node has both flags true, so this is exactly
    /// [`PartRates::bits`] there.
    #[inline]
    pub(crate) fn bits_edge(
        &self,
        ctx_row: usize,
        p: PartitionType,
        has_rows: bool,
        has_cols: bool,
    ) -> u64 {
        debug_assert!(ctx_row < 16);
        if has_rows && has_cols {
            return self.rows[ctx_row][p as usize] as u64;
        }
        if !has_rows && !has_cols {
            return 0;
        }
        let table = if !has_rows {
            &self.vert_alike
        } else {
            &self.horz_alike
        };
        table[ctx_row][usize::from(p == PartitionType::Split)] as u64
    }
}

// ---------------------------------------------------------------------------
// NSQ geometry + search controls (C NsqGeomCtrls / NsqSearchCtrls)
// ---------------------------------------------------------------------------

/// The still-funnel NSQ controls: geometry level 2 fields
/// (`svt_aom_set_nsq_geom_ctrls`, enc_mode_config.c:6408 — min_nsq 0,
/// allow_HV4 1, allow_HVA_HVB 0 at M0..M3) + the `set_nsq_search_ctrls`
/// (:6464) level fields after the tail adjustments:
/// `nsq_qp_based_th_scaling = 0` for allintra <= M3
/// (set_qp_based_th_scaling_ctrls_all_intra, enc_handle.c:4085) so
/// component/split thresholds stay RAW, and the unconditional
/// `max_part0_to_part1_dev -= 5` offset (:6797-6801, offset scaled by the
/// same disabled factors).
///
/// The runtime values were capture-verified per cell (NSQCFG rows,
/// docs/captures/nsq_m2m3/): M3 lvl 19/18/16 at qp 20/40/55, M2 lvl
/// 17/16/14.
pub(crate) struct NsqCfg {
    pub enabled: bool,
    pub min_nsq: usize,
    pub allow_hv4: bool,
    pub allow_hva_hvb: bool,
    pub sq_weight: u64,
    pub hv_weight: u64,
    pub max_part0_to_part1_dev: u64,
    pub nsq_split_cost_th: u64,
    pub lower_depth_split_cost_th: u64,
    pub h_vs_v_split_rate_th: u64,
    pub non_hv_split_rate_th: u64,
    pub rate_th_offset_lte16: u64,
    /// `psq_txs_lvl` != 0 (levels 17..19 use lvl 1: hv_to_sq_th 1000,
    /// h_to_v_th 100 — set_sq_txs_ctrls case 1, enc_mode_config.c:5266).
    pub psq_txs: bool,
    pub component_multiple_th: u64,
}

impl NsqCfg {
    /// Disabled (presets >= 4 or non-funnel paths).
    pub(crate) fn off() -> Self {
        NsqCfg {
            enabled: false,
            min_nsq: 0,
            allow_hv4: false,
            allow_hva_hvb: false,
            sq_weight: u64::MAX,
            hv_weight: u64::MAX,
            max_part0_to_part1_dev: 0,
            nsq_split_cost_th: 0,
            lower_depth_split_cost_th: 0,
            h_vs_v_split_rate_th: 0,
            non_hv_split_rate_th: 0,
            rate_th_offset_lte16: 0,
            psq_txs: false,
            component_multiple_th: 0,
        }
    }

    /// `set_nsq_search_ctrls` (enc_mode_config.c:6496) driven by whichever
    /// `pcs->nsq_search_level` ladder this frame's arm selects, and by
    /// `svt_aom_set_nsq_geom_ctrls` (:8180) for the `(allow_HV4, min_nsq)`
    /// pair that `shapes_for_size` consumes.
    ///
    /// `me_dist_mod` (:6497-6506) is 0 on an I-slice, so the `+1` ME-distortion
    /// bump never applies to any frame this port encodes — key frames are the
    /// only ones it emits.
    #[cfg(test)]
    pub(crate) fn for_arm(arm: crate::sc_detect::ScArm, preset: i8, cli_qp: u32) -> Self {
        Self::for_arm_with_coeff(
            arm,
            preset,
            cli_qp,
            crate::quant::CoeffLvl::Normal,
            0,
            crate::reference::SvtReference::Hybrid3115,
        )
    }

    pub(crate) fn for_arm_with_coeff(
        arm: crate::sc_detect::ScArm,
        preset: i8,
        cli_qp: u32,
        coeff_level: crate::quant::CoeffLvl,
        // C `pcs->temporal_layer_index` — the nsq_search_level ladder reads it
        // (`get_nsq_search_level_default`, enc_mode_config.c:8258).
        temporal_layer: u8,
        reference: crate::reference::SvtReference,
    ) -> Self {
        Self::for_levels(
            crate::part_arm::nsq_search_level_with_coeff(
                arm,
                preset,
                cli_qp,
                coeff_level,
                temporal_layer,
                reference,
            ),
            crate::part_arm::nsq_geom_level(arm, preset, reference),
            crate::part_arm::nsq_qp_based_th_scaling(arm, preset),
            cli_qp,
        )
    }

    /// `for_arm_with_coeff` plus the per-SUPERBLOCK `me_dist_mod` bump that
    /// `set_nsq_search_ctrls` applies between `pcs->nsq_search_level` and the
    /// controls row (enc_mode_config.c:4967-4984): on an inter frame
    /// (`slice_type != I_SLICE && enc_mode > ENC_MR` — every non-key frame at
    /// M0+, so `me_stats == None` is exactly the `me_dist_mod == 0` arm) the
    /// level gains +1, capped at 19, when the superblock's
    /// `me_8x8_distortion <= super_block_size^2 * 3` AND its
    /// `me_8x8_cost_variance <= 10000`. The caller resolves `(dist_8, var)`
    /// C-side per SB — `me_8x8_*[sb_index]` at `super_block_size == 64`, the
    /// `get_sb128_me_data` quadrant aggregate at 128 (:62-114).
    ///
    /// The `mimic_only_tx_4x4` arm (:4967) never reaches here: a
    /// `coded_lossless` frame takes `NsqCfg::off()` at the call site, which is
    /// level 0 with none of the switch tail.
    pub(crate) fn for_arm_sb(
        arm: crate::sc_detect::ScArm,
        preset: i8,
        cli_qp: u32,
        coeff_level: crate::quant::CoeffLvl,
        temporal_layer: u8,
        sb_size: usize,
        me_stats: Option<(u32, u32)>,
        reference: crate::reference::SvtReference,
    ) -> Self {
        let mut level = crate::part_arm::nsq_search_level_with_coeff(
            arm,
            preset,
            cli_qp,
            coeff_level,
            temporal_layer,
            reference,
        );
        if let Some((dist_8, cost_var)) = me_stats {
            if level != 0
                && u64::from(dist_8) <= (sb_size * sb_size * 3) as u64
                && cost_var <= 10000
            {
                level = level.saturating_add(1).min(19);
            }
        }
        Self::for_levels(
            level,
            crate::part_arm::nsq_geom_level(arm, preset, reference),
            crate::part_arm::nsq_qp_based_th_scaling(arm, preset),
            cli_qp,
        )
    }

    /// The `nsq_search_level` -> controls row plus the `nsq_geom_level` ->
    /// shape-gate pair. Split out of `for_arm` so the wiring and the
    /// arm-parity tests drive the SAME function.
    pub(crate) fn for_levels(
        level: u8,
        geom_level: u8,
        nsq_qp_based_th_scaling: bool,
        cli_qp: u32,
    ) -> Self {
        if level == 0 || geom_level == 0 {
            return Self::off();
        }
        let level = i32::from(level);
        let (allow_hv4, min_nsq) = crate::part_arm::nsq_geom_shape_ctrls(geom_level);

        // set_nsq_search_ctrls level rows (enc_mode_config.c:6496-6786).
        // The allintra arm reaches 2..=19 (M0's base 3 minus the qp>59 offset
        // is the floor); the VIDEO arm also reaches 1, from M0's base 2 under
        // the same offset — level 0 short-circuits above, so the `unreachable`
        // below still means what it says.
        // (sq_w, max_dev, split_th, lower_th, hvv, nonhv, off16, psq, comp, hv_w)
        let row: (u64, u64, u64, u64, u64, u64, u64, u8, u64, u64) = match level {
            1 => (105, 0, 0, 0, 0, 0, 0, 0, 0, 115),
            2 => (105, 0, 150, 3, 0, 0, 10, 0, 0, 115),
            3 => (105, 0, 100, 3, 0, 0, 10, 0, 0, 115),
            4 => (100, 0, 100, 3, 0, 0, 10, 0, 80, 115),
            5 => (100, 0, 100, 5, 0, 0, 10, 0, 80, 110),
            6 => (100, 0, 100, 5, 0, 0, 10, 0, 80, 100),
            7 => (95, 0, 80, 5, 0, 0, 10, 0, 80, 100),
            8 => (95, 0, 80, 5, 30, 20, 10, 0, 80, 100),
            9 => (95, 0, 80, 5, 40, 30, 10, 0, 60, 100),
            10 => (95, 0, 60, 10, 40, 30, 10, 0, 60, 100),
            11 => (95, 0, 60, 10, 50, 30, 10, 0, 40, 100),
            12 => (95, 0, 60, 10, 50, 30, 10, 0, 20, 100),
            13 => (95, 0, 60, 10, 60, 40, 10, 0, 20, 100),
            14 => (95, 5, 50, 10, 60, 40, 10, 0, 20, 100),
            15 => (90, 20, 40, 20, 60, 50, 10, 0, 15, 75),
            16 => (90, 50, 40, 20, 70, 60, 10, 0, 15, 75),
            17 => (90, 50, 40, 20, 70, 60, 15, 1, 10, 75),
            18 => (90, 75, 40, 20, 80, 70, 15, 1, 5, 75),
            19 => (90, 80, 35, 20, 85, 70, 15, 1, 5, 75),
            _ => unreachable!("nsq search level {level}"),
        };
        // Tail (:7110-7121). `scs->qp_based_th_scaling_ctrls.nsq_qp_based_th_scaling`
        // is 0 on the allintra arm through M3 — the only allintra band that
        // reaches here, since `get_nsq_search_level_allintra` is 0 from M4 up —
        // so the still path keeps `q_weight/q_weight_denom = 1/1` and only the
        // flat `-5` dev offset lands, exactly as before. The VIDEO arm sets the
        // flag at every reachable preset (`set_qp_based_th_scaling_ctrls_default`,
        // enc_handle.c:3806-3816, the `enc_mode > ENC_MR` arm), so there the
        // three scaled terms are live.
        let (qw, qwd) = crate::port_enc_mode_config::me::get_qp_based_th_scaling_factors(
            nsq_qp_based_th_scaling,
            cli_qp,
        );
        let scale = |v: u64| {
            crate::port_enc_mode_config::me::divide_and_round(
                u32::try_from(v * u64::from(qw)).unwrap_or(u32::MAX),
                qwd,
            ) as u64
        };
        let dev = row.1.saturating_sub(scale(5));
        NsqCfg {
            enabled: true,
            min_nsq,
            allow_hv4,
            allow_hva_hvb: geom_level == 1,
            sq_weight: row.0,
            max_part0_to_part1_dev: dev,
            nsq_split_cost_th: scale(row.2),
            lower_depth_split_cost_th: row.3,
            h_vs_v_split_rate_th: row.4,
            non_hv_split_rate_th: row.5,
            rate_th_offset_lte16: row.6,
            psq_txs: row.7 != 0,
            component_multiple_th: scale(row.8),
            hv_weight: row.9,
        }
    }
}

/// C `set_blocks_to_test`: normal shapes followed by HA/HB/VA/VB at
/// geometry level 1. The caller applies incomplete-frame restrictions.
pub(super) fn shapes_for_size(size: usize, nsq: &NsqCfg) -> &'static [PartitionType] {
    const N_ONLY: [PartitionType; 1] = [PartitionType::None];
    const NHV: [PartitionType; 3] = [
        PartitionType::None,
        PartitionType::Horz,
        PartitionType::Vert,
    ];
    const NHV4: [PartitionType; 5] = [
        PartitionType::None,
        PartitionType::Horz,
        PartitionType::Vert,
        PartitionType::Horz4,
        PartitionType::Vert4,
    ];
    const NHV_AB: [PartitionType; 7] = [
        PartitionType::None,
        PartitionType::Horz,
        PartitionType::Vert,
        PartitionType::HorzA,
        PartitionType::HorzB,
        PartitionType::VertA,
        PartitionType::VertB,
    ];
    // C Part order differs from the coded AV1 PartitionType discriminants.
    const ALL: [PartitionType; 9] = [
        PartitionType::None,
        PartitionType::Horz,
        PartitionType::Vert,
        PartitionType::Horz4,
        PartitionType::Vert4,
        PartitionType::HorzA,
        PartitionType::HorzB,
        PartitionType::VertA,
        PartitionType::VertB,
    ];
    if !nsq.enabled || size <= nsq.min_nsq || size == 4 {
        &N_ONLY
    } else if size == 8 {
        &NHV
    } else if nsq.allow_hva_hvb {
        if !nsq.allow_hv4 || size == 128 {
            &NHV_AB
        } else {
            &ALL
        }
    } else if !nsq.allow_hv4 || size == 128 {
        &NHV
    } else {
        &NHV4
    }
}

/// Child geometry of a shape at a `size` SQ node: (dx, dy, w, h) in
/// coding order (C `partition_mi_offset` + `num_ns_per_shape`).
pub(super) fn shape_children(size: usize, p: PartitionType) -> Vec<(usize, usize, usize, usize)> {
    let half = size / 2;
    let quarter = size / 4;
    match p {
        PartitionType::None => alloc::vec![(0, 0, size, size)],
        PartitionType::Horz => alloc::vec![(0, 0, size, half), (0, half, size, half)],
        PartitionType::Vert => alloc::vec![(0, 0, half, size), (half, 0, half, size)],
        PartitionType::Horz4 => (0..4).map(|i| (0, i * quarter, size, quarter)).collect(),
        PartitionType::Vert4 => (0..4).map(|i| (i * quarter, 0, quarter, size)).collect(),
        PartitionType::HorzA => alloc::vec![
            (0, 0, half, half),
            (half, 0, half, half),
            (0, half, size, half)
        ],
        PartitionType::HorzB => alloc::vec![
            (0, 0, size, half),
            (0, half, half, half),
            (half, half, half, half)
        ],
        PartitionType::VertA => alloc::vec![
            (0, 0, half, half),
            (0, half, half, half),
            (half, 0, half, size)
        ],
        PartitionType::VertB => alloc::vec![
            (0, 0, half, size),
            (half, 0, half, half),
            (half, half, half, half)
        ],
        other => unreachable!("funnel shape {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// The PD1 depth walk
// ---------------------------------------------------------------------------
