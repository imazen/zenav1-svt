//! The `scs->allintra` fork for `pcs->txs_level` — the transform-SIZE
//! (tx-partitioning) search depth.
//!
//! Sibling of [`crate::rate_arm`], [`crate::part_arm`], [`crate::intra_arm`],
//! [`crate::funnel_arm`] and [`crate::nic_arm`]. Ladder pair at
//! `enc_mode_config.c:10017` (allintra) and `:9177` (video); control table
//! `set_txs_controls` (`:6163`).
//!
//! On a KEY frame (`is_islice`, `is_base`), at `seq_qp_mod = 2`:
//!
//! | preset | allintra | video |
//! |---|---|---|
//! | 0..=1 | 2 | 2 |
//! | 2..=3 | 2 | 3 |
//! | 4..=7 | 3 | 3 |
//! | 8 | **0** | 3 |
//! | 9 | 0 | **4** |
//! | 10..=13 | 0 | **0** (clamped M11) |
//!
//! Two things the arms do NOT share:
//!
//! - **The VLPD0 coupling is allintra-only.** `svt_aom_sig_deriv_enc_dec_allintra`
//!   (`:8114`) promotes `txs_level == 0` to `MAX_TXS_LEVEL - 1` for any SB the
//!   PD0 detector leaves at `PD0_LVL_6`; the video twin
//!   (`svt_aom_sig_deriv_enc_dec_default`, `:7885`) calls `set_txs_controls`
//!   with `pcs->txs_level` unmodified. `FunnelCfg::txs_lvl6_gate` carries the
//!   per-SB half of that, so this module leaves the allintra level-0 rows
//!   alone and clears the gate on the video arm.
//! - **`frm_hdr->tx_mode`.** The allintra arm sets `TX_MODE_SELECT`
//!   unconditionally, with a comment saying it does so "even when
//!   txs_level == 0, as the decision may change from OFF to Fastest at the SB
//!   level" — the same coupling. The video arm sets
//!   `TX_MODE_SELECT` only when `txs_level != 0` (`:9194`), i.e.
//!   `TX_MODE_LARGEST` at presets 10 and up. [`tx_mode_select`] is that bit.
//!
//! # Evidence
//!
//! Tier 1 on the ladder: `pcs->txs_level` is read back on BOTH arms by
//! `tests/c_parity_sig_deriv_md_config.rs` (`TXS` slot). The control table is
//! transcribed — `set_txs_controls` writes into a `ModeDecisionContext` — and
//! pinned entry-for-entry against `FunnelCfg::for_preset`'s baked rows by
//! `allintra_flattening_matches_the_ladder`.

use crate::leaf_funnel::FunnelCfg;
use crate::port_enc_mode_config::enc_mode::{M1, M8, M9};
use crate::sc_detect::ScArm;

/// C `scs->seq_qp_mod`, set unconditionally to 2 at `enc_handle.c:3994`
/// (the same constant `part_arm::SEQ_QP_MOD` records).
const SEQ_QP_MOD: u8 = crate::part_arm::SEQ_QP_MOD;

/// `pcs->txs_level` for this arm, INCLUDING the qp banding both ladders apply
/// afterwards (`:9188` / the allintra twin has none — see below).
///
/// `enc_mode` must already be [`crate::rate_arm::eff_enc_mode`]-clamped.
#[must_use]
pub(crate) fn txs_level(arm: ScArm, enc_mode: u8, is_base: bool, cli_qp: u32) -> u8 {
    let m = i8::try_from(enc_mode).unwrap_or(i8::MAX);
    match arm {
        // enc_mode_config.c:10017. No qp banding on this arm.
        ScArm::Allintra => {
            if m <= 3 {
                2
            } else if m <= 7 {
                3
            } else {
                0
            }
        }
        // enc_mode_config.c:9177 + the qp banding at :9188.
        ScArm::Video { .. } => {
            let mut lvl: u8 = if m <= M1 {
                2
            } else if m <= M8 {
                if is_base { 3 } else { 0 }
            } else if m <= M9 {
                if is_base { 4 } else { 0 }
            } else {
                0
            };
            if lvl != 0 && SEQ_QP_MOD != 0 && cli_qp > 58 && (SEQ_QP_MOD == 1 || SEQ_QP_MOD == 2) {
                lvl = if lvl == 1 { lvl } else { lvl - 1 };
            }
            lvl
        }
    }
}

/// `frm_hdr->tx_mode == TX_MODE_SELECT` for this arm.
///
/// Allintra: always (`:10025`). Video: `txs_level != 0` (`:9194`).
#[must_use]
pub(crate) fn tx_mode_select(arm: ScArm, enc_mode: u8, is_base: bool, cli_qp: u32) -> bool {
    match arm {
        ScArm::Allintra => true,
        ScArm::Video { .. } => txs_level(arm, enc_mode, is_base, cli_qp) != 0,
    }
}

/// One `set_txs_controls` row, restricted to what [`FunnelCfg`] holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TxsRow {
    pub(crate) enabled: bool,
    pub(crate) prev_depth_coeff_exit_th: u32,
    pub(crate) intra_max_depth_sq: u8,
    pub(crate) intra_max_depth_nsq: u8,
    pub(crate) inter_max_depth_sq: u8,
    pub(crate) inter_max_depth_nsq: u8,
    pub(crate) depth1_txt_group_offset: i32,
    pub(crate) depth2_txt_group_offset: i32,
    pub(crate) quadrant_th_sf: u64,
}

/// `set_txs_controls` (`enc_mode_config.c:6163`).
///
/// The `pcs->mimic_only_tx_4x4` override at the top is not modelled here:
/// `FunnelCfg::apply_coded_lossless` already forces the level-1 row for a
/// coded-lossless frame, and it runs AFTER this module stamps.
///
/// # Panics
/// On a level outside 0..=5 — C's `default` arm silently disables, but the
/// ladders above cannot produce one and a silent disable would hide a ladder
/// bug.
#[must_use]
pub(crate) fn txs_ctrls(level: u8) -> TxsRow {
    let off = TxsRow {
        enabled: false,
        prev_depth_coeff_exit_th: 0,
        intra_max_depth_sq: 0,
        intra_max_depth_nsq: 0,
        inter_max_depth_sq: 0,
        inter_max_depth_nsq: 0,
        depth1_txt_group_offset: 0,
        depth2_txt_group_offset: 0,
        quadrant_th_sf: 0,
    };
    match level {
        0 => off,
        1 => TxsRow {
            enabled: true,
            prev_depth_coeff_exit_th: 1,
            intra_max_depth_sq: 2,
            intra_max_depth_nsq: 2,
            inter_max_depth_sq: 2,
            inter_max_depth_nsq: 2,
            ..off
        },
        2 => TxsRow {
            enabled: true,
            prev_depth_coeff_exit_th: 1,
            intra_max_depth_sq: 2,
            intra_max_depth_nsq: 2,
            inter_max_depth_sq: 1,
            inter_max_depth_nsq: 1,
            ..off
        },
        3 => TxsRow {
            enabled: true,
            prev_depth_coeff_exit_th: 1,
            intra_max_depth_sq: 1,
            intra_max_depth_nsq: 0,
            inter_max_depth_sq: 1,
            inter_max_depth_nsq: 0,
            depth1_txt_group_offset: 3,
            depth2_txt_group_offset: 3,
            ..off
        },
        4 => TxsRow {
            enabled: true,
            prev_depth_coeff_exit_th: 2,
            intra_max_depth_sq: 1,
            intra_max_depth_nsq: 1,
            inter_max_depth_sq: 0,
            inter_max_depth_nsq: 0,
            depth1_txt_group_offset: 4,
            depth2_txt_group_offset: 4,
            quadrant_th_sf: 100,
        },
        5 => TxsRow {
            enabled: true,
            prev_depth_coeff_exit_th: 100,
            intra_max_depth_sq: 1,
            intra_max_depth_nsq: 1,
            inter_max_depth_sq: 0,
            inter_max_depth_nsq: 0,
            depth1_txt_group_offset: 4,
            depth2_txt_group_offset: 4,
            quadrant_th_sf: 100,
        },
        _ => panic!("txs level {level} outside C's switch"),
    }
}

/// Stamp the row onto a [`FunnelCfg`].
///
/// On the ALLINTRA arm a level of 0 is left alone on purpose: `for_preset`'s
/// `_ =>` row encodes the VLPD0 per-SB promotion (`txs_lvl6_gate`), which is
/// not expressible as a picture-level row. Every non-zero allintra level is
/// stamped and pinned against the baked value.
pub(crate) fn apply(cfg: &mut FunnelCfg, arm: ScArm, enc_mode: u8, is_base: bool, cli_qp: u32) {
    let lvl = txs_level(arm, enc_mode, is_base, cli_qp);
    if matches!(arm, ScArm::Allintra) && lvl == 0 {
        return;
    }
    let r = txs_ctrls(lvl);
    cfg.txs_on = r.enabled;
    cfg.txs_max_sq = r.intra_max_depth_sq;
    cfg.txs_max_nsq = r.intra_max_depth_nsq;
    cfg.txs_inter_max_sq = r.inter_max_depth_sq;
    cfg.txs_inter_max_nsq = r.inter_max_depth_nsq;
    cfg.txt_d1_off = r.depth1_txt_group_offset;
    cfg.txt_d2_off = r.depth2_txt_group_offset;
    cfg.txs_prev_depth_exit = r.prev_depth_coeff_exit_th;
    cfg.txs_quadrant_sf = r.quadrant_th_sf;
    if matches!(arm, ScArm::Video { .. }) {
        // The VLPD0 promotion is allintra-only (module doc).
        cfg.txs_lvl6_gate = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The still path, entry-for-entry against `FunnelCfg::for_preset`.
    ///
    /// Presets 8..=13 land on allintra `txs_level = 0`, where [`apply`] is a
    /// no-op by construction, so only the LEVEL is asserted there.
    #[test]
    fn allintra_flattening_matches_the_ladder() {
        for preset in 0u8..=13 {
            for cli_qp in [0u32, 20, 40, 55, 59, 63] {
                let baked = FunnelCfg::for_preset(preset);
                let mut walked = baked;
                let eff = crate::rate_arm::eff_enc_mode(ScArm::Allintra, preset);
                let lvl = txs_level(ScArm::Allintra, eff, true, cli_qp);
                assert_eq!(
                    lvl,
                    if preset <= 3 {
                        2
                    } else if preset <= 7 {
                        3
                    } else {
                        0
                    },
                    "allintra txs_level M{preset} q{cli_qp}"
                );
                apply(&mut walked, ScArm::Allintra, eff, true, cli_qp);
                let f = |c: &FunnelCfg| {
                    (
                        c.txs_on,
                        c.txs_max_sq,
                        c.txs_max_nsq,
                        c.txs_inter_max_sq,
                        c.txs_inter_max_nsq,
                        c.txt_d1_off,
                        c.txt_d2_off,
                        c.txs_prev_depth_exit,
                        c.txs_quadrant_sf,
                        c.txs_lvl6_gate,
                    )
                };
                assert_eq!(
                    f(&baked),
                    f(&walked),
                    "allintra txs ladder vs FunnelCfg::for_preset at M{preset} q{cli_qp}"
                );
            }
        }
    }

    /// The video rows the campaign's cells stand on, and the `tx_mode` bit.
    #[test]
    fn video_key_frame_txs_rows() {
        let arm = ScArm::Video { is_islice: true };
        assert_eq!(txs_level(arm, 6, true, 40), 3);
        assert_eq!(txs_level(arm, 9, true, 40), 4);
        // The qp banding drops a level above cli_qp 58.
        assert_eq!(txs_level(arm, 9, true, 59), 3);
        // Clamped M11 -> level 0 -> TX_MODE_LARGEST, where the allintra arm
        // signals TX_MODE_SELECT at every preset.
        assert_eq!(txs_level(arm, 11, true, 40), 0);
        assert!(!tx_mode_select(arm, 11, true, 40));
        assert!(tx_mode_select(ScArm::Allintra, 11, true, 40));
        assert!(tx_mode_select(arm, 9, true, 40));
    }
}
