//! `svt_aom_sig_deriv_enc_dec_pd0` (`Codec/enc_mode_config.c:7207`) — the
//! per-SB PD0 signal set, shared by ALL THREE arms (allintra included).
//!
//! The partition tree PD0 picks is the first thing an inter SB's bytes depend
//! on, and this function's absence was a still-path gap too, not only an inter
//! one: the port hardcodes the values it would produce.
//!
//! **Tier 1** — the entry point is EXPORTED and
//! `c_parity_sig_deriv_pd0.rs` drives the real symbol.

use super::encdec::{
    DepthEarlyExitCtrls, PfCtrls, SubresCtrls, set_depth_early_exit_ctrls, set_pf_controls,
    set_subres_controls,
};
use super::leaf::MAX_INTRA_LEVEL;
use super::tail::{compute_intra_pd0_th, compute_subres_th, rdcost};

/// C `Pd0Level` (`definitions.h:762`).
pub mod pd0_level {
    /// `PD0_LVL_0`
    pub const L0: u8 = 0;
    /// `PD0_LVL_1`
    pub const L1: u8 = 1;
    /// `PD0_LVL_2`
    pub const L2: u8 = 2;
    /// `PD0_LVL_3`
    pub const L3: u8 = 3;
    /// `PD0_LVL_4`
    pub const L4: u8 = 4;
    /// `PD0_LVL_5`
    pub const L5: u8 = 5;
    /// `PD0_LVL_6` — the lightest PD0 path; does not perform TX.
    pub const L6: u8 = 6;
}

/// C `MdRateEstCtrls` (`md_process.h:763`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MdRateEstCtrls {
    /// `update_skip_ctx_dc_sign_ctx`
    pub update_skip_ctx_dc_sign_ctx: bool,
    /// `update_skip_coeff_ctx`
    pub update_skip_coeff_ctx: bool,
    /// `coeff_rate_est_lvl`
    pub coeff_rate_est_lvl: u8,
    /// `lpd0_qp_offset`
    pub lpd0_qp_offset: i8,
    /// `pd0_fast_coeff_est_level`
    pub pd0_fast_coeff_est_level: u8,
}

/// C `set_rate_est_ctrls` (`enc_mode_config.c:6428`). static.
///
/// Level 0 is NOT an all-zero struct: it sets `lpd0_qp_offset = 8` and
/// `pd0_fast_coeff_est_level = 2`.
#[must_use]
pub fn set_rate_est_ctrls(rate_est_level: u8) -> Option<MdRateEstCtrls> {
    match rate_est_level {
        0 => Some(MdRateEstCtrls {
            update_skip_ctx_dc_sign_ctx: false,
            update_skip_coeff_ctx: false,
            coeff_rate_est_lvl: 0,
            lpd0_qp_offset: 8,
            pd0_fast_coeff_est_level: 2,
        }),
        1 => Some(MdRateEstCtrls {
            update_skip_ctx_dc_sign_ctx: true,
            update_skip_coeff_ctx: true,
            coeff_rate_est_lvl: 1,
            lpd0_qp_offset: 0,
            pd0_fast_coeff_est_level: 1,
        }),
        2 => Some(MdRateEstCtrls {
            update_skip_ctx_dc_sign_ctx: true,
            update_skip_coeff_ctx: false,
            coeff_rate_est_lvl: 1,
            lpd0_qp_offset: 0,
            pd0_fast_coeff_est_level: 2,
        }),
        3 => Some(MdRateEstCtrls {
            update_skip_ctx_dc_sign_ctx: true,
            update_skip_coeff_ctx: false,
            coeff_rate_est_lvl: 2,
            lpd0_qp_offset: 0,
            pd0_fast_coeff_est_level: 2,
        }),
        4 => Some(MdRateEstCtrls {
            update_skip_ctx_dc_sign_ctx: false,
            update_skip_coeff_ctx: false,
            coeff_rate_est_lvl: 2,
            lpd0_qp_offset: 0,
            pd0_fast_coeff_est_level: 2,
        }),
        _ => None,
    }
}

/// C `CLIP3(min, max, a)` (`Codec/utility.h:101`).
#[must_use]
pub const fn clip3(min_val: i64, max_val: i64, a: i64) -> i64 {
    if a < min_val {
        min_val
    } else if a > max_val {
        max_val
    } else {
        a
    }
}

/// Everything `svt_aom_sig_deriv_enc_dec_pd0` reads off the SCS / PCS / PPCS /
/// context, spelled out.
#[derive(Debug, Clone, Copy)]
pub struct Pd0Inputs {
    /// `ctx->pd0_ctrls.pd0_level`
    pub pd0_level: u8,
    /// `pcs->slice_type == I_SLICE`
    pub is_islice: bool,
    /// `scs->allintra`
    pub allintra: bool,
    /// `scs->static_config.rtc`
    pub rtc_tune: bool,
    /// `!frame_is_leaf(ppcs)`
    pub is_not_last_layer: bool,
    /// `pcs->enc_mode`
    pub enc_mode: i8,
    /// `ppcs->transition_present == 1`
    pub transition_present: bool,
    /// `ctx->pic_pred_depth_only`
    pub pic_pred_depth_only: bool,
    /// `ctx->hbd_md`
    pub ctx_hbd_md: bool,
    /// `pcs->hbd_md`
    pub pcs_hbd_md: bool,
    /// `ctx->fast_lambda_md[EB_8_BIT_MD]`
    pub fast_lambda_8bit: u32,
    /// `ctx->fast_lambda_md[EB_10_BIT_MD]`
    pub fast_lambda_10bit: u32,
    /// `ppcs->me_64x64_distortion[ctx->sb_index]`
    pub me_64x64_distortion: u32,
    /// `ppcs->me_8x8_cost_variance[ctx->sb_index]`
    pub me_8x8_cost_variance: u32,
    /// `ppcs->me_8x8_distortion[ctx->sb_index]`
    pub me_8x8_distortion: u32,
    /// `ppcs->frm_hdr.quantization_params.base_q_idx`
    pub base_q_idx: u32,
    /// `pcs->pd0_cost_bias_weight`
    pub pd0_cost_bias_weight: u32,
    /// `pcs->rate_est_level`
    pub rate_est_level: u8,
    /// `ctx->disallow_4x4`
    pub disallow_4x4: bool,
    /// `ctx->disallow_8x8`
    pub disallow_8x8: bool,
    /// `ctx->depth_removal_ctrls.enabled`
    pub depth_removal_enabled: bool,
    /// `ctx->depth_removal_ctrls.disallow_below_16x16`
    pub disallow_below_16x16: bool,
    /// `ctx->depth_removal_ctrls.disallow_below_32x32`
    pub disallow_below_32x32: bool,
    /// `ctx->depth_removal_ctrls.disallow_below_64x64`
    pub disallow_below_64x64: bool,
    /// `ppcs->b64_geom[ctx->sb_index].is_complete_b64`
    pub b64_is_complete: bool,
    /// `scs->super_block_size`
    pub super_block_size: u32,
}

/// What `svt_aom_sig_deriv_enc_dec_pd0` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pd0Signals {
    /// `ctx->md_disallow_nsq_search` — hardcoded 1.
    pub md_disallow_nsq_search: u8,
    /// `ctx->shut_fast_rate` — hardcoded true.
    pub shut_fast_rate: bool,
    /// The `depth_early_exit_lvl` this function derives.
    pub depth_early_exit_lvl: u8,
    /// `ctx->depth_early_exit_ctrls`
    pub depth_early_exit: DepthEarlyExitCtrls,
    /// The `intra_level` this function derives and passes to
    /// `set_intra_ctrls(pcs, ctx, intra_level, 2)`.
    ///
    /// The resulting `ctx->intra_ctrls` is NOT modelled here (that table is
    /// unported); the differential validates this level by feeding it through
    /// C's own `set_intra_ctrls` from a second exported entry point.
    pub intra_level: u8,
    /// `ctx->parent_cost_bias`
    pub parent_cost_bias: u16,
    /// `ctx->pd0_use_src_samples`
    pub pd0_use_src_samples: bool,
    /// True when C returned early at `PD0_LVL_6`; everything below this point
    /// is then left at the context's prior value.
    pub returned_early: bool,
    /// `ctx->pf_ctrls` (unset when `returned_early`).
    pub pf: PfCtrls,
    /// The `subres_level` this function derives (0 when `returned_early`).
    pub subres_level: u8,
    /// `ctx->subres_ctrls` (unset when `returned_early`).
    pub subres: SubresCtrls,
    /// The `rate_est_level` this function derives (0 when `returned_early`).
    pub rate_est_level: u8,
    /// `ctx->rate_est_ctrls` (unset when `returned_early`).
    pub rate_est: MdRateEstCtrls,
    /// `ctx->approx_inter_rate` (unset when `returned_early`).
    pub approx_inter_rate: u8,
}

/// C `svt_aom_sig_deriv_enc_dec_pd0` (`enc_mode_config.c:7207`). EXPORTED.
///
/// Returns `None` only when a derived level lands outside its table's domain,
/// which C would `assert(0)` on.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn sig_deriv_enc_dec_pd0(i: Pd0Inputs) -> Option<Pd0Signals> {
    let pd0_level = i.pd0_level;
    let fast_lambda = if i.ctx_hbd_md {
        i.fast_lambda_10bit
    } else {
        i.fast_lambda_8bit
    };
    // The SB128 branch calls get_sb128_me_data; on a 64x64 SB the distortion
    // comes straight out of the per-SB array.
    let me_64x64_dist = i.me_64x64_distortion;

    // Depth early exit level.
    let depth_early_exit_lvl = if i.rtc_tune && pd0_level == pd0_level::L6 {
        0
    } else if pd0_level <= pd0_level::L1 || i.pic_pred_depth_only {
        1
    } else {
        2
    };

    // Intra level.
    let preset_bound = if i.rtc_tune { 9 } else { 10 };
    let intra_level: u8 = if i.enc_mode <= preset_bound {
        if pd0_level == pd0_level::L0 {
            (MAX_INTRA_LEVEL - 1) as u8
        } else if i.is_islice || i.transition_present {
            1
        } else if pd0_level <= pd0_level::L2 {
            let use_intra_pd0_th = compute_intra_pd0_th(fast_lambda, i.super_block_size);
            let cost_64x64 = rdcost(i64::from(fast_lambda), 0, i64::from(me_64x64_dist)) as u64;
            u8::from(cost_64x64 >= use_intra_pd0_th)
        } else {
            0
        }
    } else if i.is_islice || i.transition_present {
        8
    } else {
        0
    };

    // Parent cost bias.
    let mut parent_cost_bias: i64 = 1000;
    if !i.allintra && pd0_level == pd0_level::L6 {
        // QP component: linear interpolation from 1100 (q=0) to 950 (q=255).
        parent_cost_bias = 1100 - i64::from(i.base_q_idx * 150 + 127) / 255;
        let me_var = i.me_8x8_cost_variance;
        if i.pd0_cost_bias_weight != 0 {
            let dist_64 = i.me_64x64_distortion;
            let dist_8 = i.me_8x8_distortion.max(1);
            let ratio_q4 = (dist_64 * 16) / dist_8;
            // NOTE: `(ratio_q4 - 16)` is UNSIGNED arithmetic in C, so a
            // ratio below 16 wraps to a huge value and CLIP3 pins it at 1024.
            let w = clip3(
                i64::from(i.pd0_cost_bias_weight),
                1024,
                i64::from(
                    ratio_q4
                        .wrapping_sub(16)
                        .wrapping_mul(16)
                        .wrapping_add(i.pd0_cost_bias_weight),
                ),
            );
            if me_var > 2000 {
                parent_cost_bias += (75 * w) >> 10;
            } else if me_var > 1000 {
                parent_cost_bias += (50 * w) >> 10;
            } else if me_var > 500 {
                parent_cost_bias += (25 * w) >> 10;
            }
        } else if me_var > 2000 {
            parent_cost_bias += 75;
        } else if me_var > 1000 {
            parent_cost_bias += 50;
        } else if me_var > 500 {
            parent_cost_bias += 25;
        }
        parent_cost_bias = clip3(900, 1200, parent_cost_bias);
    }

    let pd0_use_src_samples = i.allintra || i.pcs_hbd_md;

    let mut s = Pd0Signals {
        md_disallow_nsq_search: 1,
        shut_fast_rate: true,
        depth_early_exit_lvl,
        depth_early_exit: set_depth_early_exit_ctrls(depth_early_exit_lvl)?,
        intra_level,
        parent_cost_bias: parent_cost_bias as u16,
        pd0_use_src_samples,
        returned_early: pd0_level == pd0_level::L6,
        pf: PfCtrls::default(),
        subres_level: 0,
        subres: SubresCtrls::default(),
        rate_est_level: 0,
        rate_est: MdRateEstCtrls::default(),
        approx_inter_rate: 0,
    };
    if s.returned_early {
        return Some(s);
    }

    // C: svt_aom_set_chroma_controls(ctx, 0) — chroma off. Not modelled here
    // (that table is unported); the differential reads C's uv_mode instead.
    s.pf = set_pf_controls(1)?;

    // Sub-resolution level.
    let subres_level: u8 = if pd0_level <= pd0_level::L2 || !i.disallow_4x4 || !i.b64_is_complete {
        0
    } else {
        let use_subres_th = compute_subres_th(fast_lambda, i.super_block_size);
        let cost_64x64 = rdcost(i64::from(fast_lambda), 0, i64::from(me_64x64_dist)) as u64;
        if pd0_level <= pd0_level::L4 {
            if i.is_islice || i.transition_present {
                1
            } else {
                u8::from(cost_64x64 < use_subres_th)
            }
        } else if i.is_not_last_layer {
            let removal_forces_2 = i.depth_removal_enabled
                && (i.disallow_below_16x16 || i.disallow_below_32x32 || i.disallow_below_64x64);
            if i.disallow_8x8 || removal_forces_2 {
                2
            } else {
                1
            }
        } else {
            2
        }
    };
    s.subres_level = subres_level;
    s.subres = set_subres_controls(subres_level)?;

    // Rate estimation level.
    let mut rate_est_level: u8 = 0;
    if i.rate_est_level != 0 {
        rate_est_level = if pd0_level <= pd0_level::L3 {
            2
        } else if pd0_level <= pd0_level::L4 {
            4
        } else {
            0
        };
        // Don't use a more conservative level in LPD0 than the regular path.
        if rate_est_level != 0 {
            rate_est_level = rate_est_level.max(i.rate_est_level);
        }
    }
    s.rate_est_level = rate_est_level;
    s.rate_est = set_rate_est_ctrls(rate_est_level)?;
    s.approx_inter_rate = 1;
    Some(s)
}
