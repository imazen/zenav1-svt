use super::*;

/// C `TfControls` (`definitions.h:155-234`) — the complete per-type parameter
/// set. `scs->tf_params_per_type[0..3]` holds the I_SLICE / BASE / L1 entries
/// the `tf_controls` / `tf_ld_controls` tables fill; `pcs->tf_ctrls` carries
/// the copy `copy_tf_params` selected for one picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TfCtrls {
    /// C `tf_ctrls.enabled`.
    pub enabled: bool,
    /// C `tf_ctrls.chroma_lvl` — 0: Y only, 1: all planes, 2: Y plus
    /// noise-gated chroma.
    pub chroma_lvl: u8,
    /// C `tf_ctrls.use_zz_based_filter` — skip ME and filter with (0,0) MVs.
    /// Only ever set by `tf_ld_controls` levels 1/2, which `derive_tf_params`
    /// never selects (see `port_temporal_filtering.rs`'s dead-arms note).
    pub use_zz_based_filter: bool,
    /// C `tf_ctrls.num_past_pics`.
    pub num_past_pics: u8,
    /// C `tf_ctrls.num_future_pics`.
    pub num_future_pics: u8,
    /// C `tf_ctrls.modulate_pics` (0 disables modulation entirely).
    pub modulate_pics: u8,
    /// C `tf_ctrls.use_intra_for_noise_est`.
    pub use_intra_for_noise_est: bool,
    /// C `tf_ctrls.max_num_past_pics`.
    pub max_num_past_pics: u8,
    /// C `tf_ctrls.max_num_future_pics`.
    pub max_num_future_pics: u8,
    /// C `tf_ctrls.hme_me_level`.
    pub hme_me_level: u8,
    /// C `tf_ctrls.half_pel_mode`.
    pub half_pel_mode: u8,
    /// C `tf_ctrls.quarter_pel_mode`.
    pub quarter_pel_mode: u8,
    /// C `tf_ctrls.eight_pel_mode`.
    pub eight_pel_mode: u8,
    /// C `tf_ctrls.use_8bit_subpel`.
    pub use_8bit_subpel: bool,
    /// C `tf_ctrls.avoid_2d_qpel`.
    pub avoid_2d_qpel: bool,
    /// C `tf_ctrls.use_2tap`.
    pub use_2tap: bool,
    /// C `tf_ctrls.sub_sampling_shift`.
    pub sub_sampling_shift: u8,
    /// C `tf_ctrls.pred_error_32x32_th`.
    pub pred_error_32x32_th: u64,
    /// C `tf_ctrls.enable_8x8_pred`.
    pub enable_8x8_pred: bool,
    /// C `tf_ctrls.me_exit_th`.
    pub me_exit_th: u32,
    /// C `tf_ctrls.use_pred_64x64_only_th`.
    pub use_pred_64x64_only_th: u8,
    /// C `tf_ctrls.subpel_early_exit_th`.
    pub subpel_early_exit_th: u8,
    /// C `tf_ctrls.ref_frame_factor`.
    pub ref_frame_factor: u8,
    /// C `tf_ctrls.qp_opt`.
    pub qp_opt: bool,
}

/// C `tf_controls` (`enc_handle.c:2646-3320`) — static.
///
/// The RANDOM_ACCESS parameter table, one switch arm per `tf_level` (0-9).
/// Returns `[I_SLICE, BASE, L1]`, C's `tf_params_per_type[0..3]`. Levels
/// 3/4/6/7/8 are unreachable through `derive_tf_params` (it emits only
/// {0,1,2,5,9}) and are transcribed anyway per `WORKING-ON-THIS.md` §7.
///
/// Every max-count is `MIN(1 << hierarchical_levels, svt_aom_tf_max_ref_per_struct(..))`
/// for BASE and `MIN((1 << hierarchical_levels) / 2, ..)` for L1; the I_SLICE
/// entry caps only its future count. `use_zz_based_filter` is forced 0 on all
/// three entries after the switch (`:3317-3319`), which `TfCtrls::default()`
/// already gives.
#[must_use]
pub fn tf_controls(hierarchical_levels: u8, tf_level: u8) -> [TfCtrls; 3] {
    let hier = u32::from(hierarchical_levels);
    let mg = (1u32 << hier) as u8;
    let mg_half = ((1u32 << hier) / 2) as u8;
    // C `MIN(1 << hier, tf_max_ref_per_struct(hier, ty, dir))` per type.
    let max_i_fut = mg.min(tf_max_ref_per_struct(hier, 0, true));
    let max_base_past = mg.min(tf_max_ref_per_struct(hier, 1, false));
    let max_base_fut = mg.min(tf_max_ref_per_struct(hier, 1, true));
    let max_l1_past = mg_half.min(tf_max_ref_per_struct(hier, 2, false));
    let max_l1_fut = mg_half.min(tf_max_ref_per_struct(hier, 2, true));

    let mut t = [TfCtrls::default(); 3];
    match tf_level {
        0 => {}
        1 => {
            let base = TfCtrls {
                enabled: true,
                hme_me_level: 1,
                half_pel_mode: 1,
                quarter_pel_mode: 1,
                eight_pel_mode: 1,
                chroma_lvl: 1,
                enable_8x8_pred: true,
                use_8bit_subpel: true,
                ref_frame_factor: 1,
                ..TfCtrls::default()
            };
            t[0] = TfCtrls {
                num_future_pics: 24,
                modulate_pics: 1,
                max_num_future_pics: max_i_fut,
                ..base
            };
            t[1] = TfCtrls {
                num_past_pics: 1,
                num_future_pics: 1,
                modulate_pics: 1,
                max_num_past_pics: max_base_past,
                max_num_future_pics: max_base_fut,
                ..base
            };
            t[2] = TfCtrls {
                num_past_pics: 1,
                num_future_pics: 1,
                modulate_pics: 1,
                max_num_past_pics: max_l1_past,
                max_num_future_pics: max_l1_fut,
                ..base
            };
        }
        2 => {
            let base = TfCtrls {
                enabled: true,
                hme_me_level: 1,
                half_pel_mode: 1,
                quarter_pel_mode: 1,
                eight_pel_mode: 1,
                chroma_lvl: 1,
                pred_error_32x32_th: 8 * 32 * 32,
                enable_8x8_pred: true,
                use_8bit_subpel: true,
                ref_frame_factor: 1,
                ..TfCtrls::default()
            };
            t[0] = TfCtrls {
                num_future_pics: 24,
                modulate_pics: 1,
                max_num_future_pics: max_i_fut,
                ..base
            };
            t[1] = TfCtrls {
                num_past_pics: 1,
                num_future_pics: 1,
                modulate_pics: 2,
                max_num_past_pics: max_base_past,
                max_num_future_pics: max_base_fut,
                ..base
            };
            t[2] = TfCtrls {
                num_past_pics: 1,
                num_future_pics: 1,
                modulate_pics: 1,
                max_num_past_pics: max_l1_past,
                max_num_future_pics: max_l1_fut,
                ..base
            };
        }
        3 => {
            let base = TfCtrls {
                enabled: true,
                hme_me_level: 1,
                half_pel_mode: 1,
                quarter_pel_mode: 1,
                eight_pel_mode: 1,
                chroma_lvl: 1,
                pred_error_32x32_th: 8 * 32 * 32,
                use_8bit_subpel: true,
                ref_frame_factor: 1,
                qp_opt: true,
                ..TfCtrls::default()
            };
            t[0] = TfCtrls {
                num_future_pics: 24,
                modulate_pics: 1,
                max_num_future_pics: max_i_fut,
                enable_8x8_pred: true,
                ..base
            };
            t[1] = TfCtrls {
                num_past_pics: 1,
                num_future_pics: 1,
                modulate_pics: 2,
                max_num_past_pics: max_base_past,
                max_num_future_pics: max_base_fut,
                ..base
            };
            t[2] = TfCtrls {
                num_past_pics: 1,
                num_future_pics: 1,
                modulate_pics: 1,
                max_num_past_pics: max_l1_past,
                max_num_future_pics: max_l1_fut,
                ..base
            };
        }
        4 => {
            let base = TfCtrls {
                enabled: true,
                hme_me_level: 2,
                half_pel_mode: 1,
                quarter_pel_mode: 1,
                chroma_lvl: 1,
                pred_error_32x32_th: 8 * 32 * 32,
                use_8bit_subpel: true,
                ref_frame_factor: 1,
                qp_opt: true,
                ..TfCtrls::default()
            };
            t[0] = TfCtrls {
                num_future_pics: 24,
                modulate_pics: 1,
                max_num_future_pics: max_i_fut,
                subpel_early_exit_th: 1,
                ..base
            };
            t[1] = TfCtrls {
                num_past_pics: 1,
                num_future_pics: 1,
                modulate_pics: 2,
                max_num_past_pics: max_base_past,
                max_num_future_pics: max_base_fut,
                subpel_early_exit_th: 1,
                ..base
            };
            // L1 alone keeps the eight-pel search and the full subpel path.
            t[2] = TfCtrls {
                num_past_pics: 1,
                num_future_pics: 1,
                modulate_pics: 1,
                max_num_past_pics: max_l1_past,
                max_num_future_pics: max_l1_fut,
                eight_pel_mode: 1,
                ..base
            };
        }
        5 => {
            let base = TfCtrls {
                enabled: true,
                hme_me_level: 2,
                half_pel_mode: 2,
                quarter_pel_mode: 1,
                chroma_lvl: 1,
                pred_error_32x32_th: 20 * 32 * 32,
                use_2tap: true,
                use_8bit_subpel: true,
                subpel_early_exit_th: 1,
                ref_frame_factor: 1,
                qp_opt: true,
                ..TfCtrls::default()
            };
            t[0] = TfCtrls {
                num_future_pics: 24,
                modulate_pics: 1,
                max_num_future_pics: max_i_fut,
                ..base
            };
            t[1] = TfCtrls {
                num_past_pics: 1,
                num_future_pics: 1,
                modulate_pics: 3,
                max_num_past_pics: max_base_past,
                max_num_future_pics: max_base_fut,
                ..base
            };
            t[2] = TfCtrls {
                num_past_pics: 1,
                num_future_pics: 1,
                modulate_pics: 2,
                max_num_past_pics: max_l1_past,
                max_num_future_pics: max_l1_fut,
                ..base
            };
        }
        6 | 7 | 8 | 9 => {
            // The fast tail: chroma off, exhaustive-error thresholds, 2-tap
            // subsampled subpel, key-frame noise reuse, and L1 disabled.
            let is8 = if hier < 5 { 8 } else { 16 };
            t[0] = TfCtrls {
                enabled: true,
                num_future_pics: if tf_level == 6 || tf_level == 7 {
                    is8
                } else {
                    8
                },
                max_num_future_pics: max_i_fut,
                hme_me_level: if tf_level == 9 { 3 } else { 2 },
                half_pel_mode: 2,
                quarter_pel_mode: 1,
                chroma_lvl: 0,
                pred_error_32x32_th: u64::MAX,
                sub_sampling_shift: 1,
                avoid_2d_qpel: true,
                use_2tap: true,
                use_intra_for_noise_est: true,
                use_8bit_subpel: true,
                use_pred_64x64_only_th: if tf_level == 6 { 0 } else { 35 },
                me_exit_th: if tf_level == 6 { 0 } else { 16 * 16 },
                subpel_early_exit_th: if tf_level <= 7 { 1 } else { 4 },
                ref_frame_factor: if tf_level <= 7 { 1 } else { 2 },
                qp_opt: true,
                ..TfCtrls::default()
            };
            t[1] = TfCtrls {
                enabled: true,
                num_past_pics: 1,
                num_future_pics: 1,
                modulate_pics: if tf_level <= 7 { 3 } else { 4 },
                max_num_past_pics: max_base_past,
                max_num_future_pics: max_base_fut,
                hme_me_level: if tf_level == 9 { 3 } else { 2 },
                half_pel_mode: 2,
                quarter_pel_mode: 1,
                chroma_lvl: if tf_level <= 7 { 1 } else { 0 },
                pred_error_32x32_th: if tf_level <= 7 {
                    20 * 32 * 32
                } else {
                    u64::MAX
                },
                sub_sampling_shift: if tf_level <= 7 { 0 } else { 1 },
                avoid_2d_qpel: tf_level >= 8,
                use_2tap: true,
                use_intra_for_noise_est: true,
                use_8bit_subpel: true,
                use_pred_64x64_only_th: if tf_level == 6 { 0 } else { 35 },
                me_exit_th: if tf_level == 6 { 0 } else { 16 * 16 },
                subpel_early_exit_th: if tf_level <= 7 { 1 } else { 4 },
                ref_frame_factor: 1,
                qp_opt: true,
                ..TfCtrls::default()
            };
            // t[2] stays disabled.
        }
        _ => unreachable!("tf_level {tf_level} is outside C's 0..=9 switch"),
    }
    t
}

/// C `tf_ld_controls` (`enc_handle.c:2525-2644`) — static.
///
/// The LOW_DELAY parameter table. `derive_tf_params` calls it only with
/// `tf_level == 0` (all disabled — "TF disabled for all LD"); levels 1/2 are
/// unreachable configuration and are the ONLY source of
/// `use_zz_based_filter = 1`, which is why the `zz` kernels in
/// `port_temporal_filtering.rs` are dead code. `enable_8x8_pred` is forced 0
/// on all three entries after the switch (`:2641-2643`).
#[must_use]
pub fn tf_ld_controls(tf_level: u8) -> [TfCtrls; 3] {
    let mut t = [TfCtrls::default(); 3];
    match tf_level {
        0 => {}
        1 | 2 => {
            t[1] = TfCtrls {
                enabled: true,
                num_past_pics: 1,
                num_future_pics: 0,
                modulate_pics: 0,
                max_num_past_pics: 1,
                max_num_future_pics: 0,
                hme_me_level: 4,
                half_pel_mode: 0,
                quarter_pel_mode: 0,
                eight_pel_mode: 0,
                chroma_lvl: if tf_level == 1 { 1 } else { 2 },
                pred_error_32x32_th: if tf_level == 1 {
                    20 * 32 * 32
                } else {
                    u64::MAX
                },
                sub_sampling_shift: 0,
                use_zz_based_filter: true,
                avoid_2d_qpel: false,
                use_2tap: false,
                use_intra_for_noise_est: false,
                use_8bit_subpel: false,
                use_pred_64x64_only_th: 0,
                me_exit_th: 0,
                subpel_early_exit_th: u8::from(tf_level == 1),
                ref_frame_factor: 1,
                qp_opt: false,
                enable_8x8_pred: false,
            };
        }
        _ => unreachable!("tf_ld level {tf_level} is outside C's 0..=2 switch"),
    }
    t
}

/// C `derive_tf_params` (`enc_handle.c:3333-3355`) — static.
///
/// Selects the TF level from the sequence knobs and fills
/// `tf_params_per_type`. Returns `(tf_level, [I_SLICE, BASE, L1])`.
///
/// * LOW_DELAY forces `tf_level = 0` through `tf_ld_controls` before any
///   preset logic — TF is inert in low delay no matter what `enable_tf` says.
/// * `do_tf = enable_tf && hierarchical_levels >= 1 && !lossless` — a
///   flat/1-layer GOP or a lossless config disables it even in random access.
/// * `enc_mode` is the CONFIGURED preset (`static_config.enc_mode`, post the
///   resource-coordination clamp): `<= M1` → 1, `<= M2` → 2, `<= M7` → 5,
///   else 9.
#[must_use]
pub fn derive_tf_params(
    pred_structure: PredStructure,
    enc_mode: i8,
    hierarchical_levels: u8,
    enable_tf: bool,
    lossless: bool,
) -> (u8, [TfCtrls; 3]) {
    use crate::port_enc_mode_config::enc_mode::{M1, M2, M7};
    if pred_structure == PredStructure::LowDelay {
        return (0, tf_ld_controls(0));
    }
    let do_tf = enable_tf && hierarchical_levels >= 1 && !lossless;
    let tf_level = if !do_tf {
        0
    } else if enc_mode <= M1 {
        1
    } else if enc_mode <= M2 {
        2
    } else if enc_mode <= M7 {
        5
    } else {
        9
    };
    (tf_level, tf_controls(hierarchical_levels, tf_level))
}

/// C `ref_pics_modulation` (`pd_process.c:3642-3745`) — static.
///
/// Modulates the temporal-filter reference count from the noise level (I
/// slices) or the filtered-vs-unfiltered intra distortion ratio (inter). It
/// changes how many pictures the filter averages, hence the SOURCE PIXELS.
///
/// Traps:
/// * the I-slice arm reads noise DIRECTLY against three Q16 log1p constants
///   (26572 = log1p(0.5), 45426 = log1p(1.0), 71998 = log1p(2.0)) and yields
///   6 / 4 / 2 / 0 — a LOWER noise level gets MORE frames, which is the
///   opposite of the intuitive direction and is what the C comment explains.
/// * the inter arms divide by the noise level, so `noise == 0` short-circuits
///   the ratio to 0 rather than dividing.
/// * base-layer and non-base use DIFFERENT `modulate_pics` tables, and the
///   non-base one has no `case 4`, so `modulate_pics == 4` falls through to
///   `offset = 0` there while base-layer gives 0/1/2.
///
/// `q_weight` / `q_weight_denom` come from
/// `svt_aom_get_qp_based_th_scaling_factors`, which belongs to the
/// signal-derivation module; the caller supplies them and they are applied
/// here only when `qp_opt`.
#[must_use]
pub fn ref_pics_modulation(
    is_i_slice: bool,
    temporal_layer_index: u8,
    ctrls: &TfCtrls,
    noise_levels_log1p_fp16: i32,
    filt_to_unfilt_diff: u32,
    q_weight: u32,
    q_weight_denom: u32,
) -> i32 {
    let mut offset: i32 = 0;

    if is_i_slice {
        // Q16 log1p thresholds; LOWER noise buys MORE filtering frames.
        if noise_levels_log1p_fp16 < 26572 {
            offset = 6;
        } else if noise_levels_log1p_fp16 < 45426 {
            offset = 4;
        } else if noise_levels_log1p_fp16 < 71998 {
            offset = 2;
        }
    } else {
        // C computes `(pcs->filt_to_unfilt_diff * 100) / noise` in UINT32:
        // the multiply wraps, the divisor sign-extends into u32, and the
        // quotient is then assigned to `int`. The `~0` carry between I
        // slices makes the wraparound VISIBLE — u32 gives a huge positive
        // ratio (max offset), a signed multiply would give 0.
        let ratio: i32 = if noise_levels_log1p_fp16 != 0 {
            (filt_to_unfilt_diff.wrapping_mul(100) / (noise_levels_log1p_fp16 as u32)) as i32
        } else {
            0
        };
        if temporal_layer_index == 0 {
            offset = match ctrls.modulate_pics {
                1 => {
                    if ratio < 100 {
                        5
                    } else {
                        TF_MAX_EXTENSION
                    }
                }
                2 => {
                    if ratio < 50 {
                        3
                    } else if ratio < 100 {
                        5
                    } else {
                        TF_MAX_EXTENSION
                    }
                }
                3 => {
                    if ratio < 50 {
                        3
                    } else if ratio < 100 {
                        4
                    } else {
                        5
                    }
                }
                4 => {
                    if ratio < 50 {
                        0
                    } else if ratio < 100 {
                        1
                    } else {
                        2
                    }
                }
                // case 0 and default.
                _ => 0,
            };
        } else {
            offset = match ctrls.modulate_pics {
                1 => i32::from(ratio >= 25),
                2 => i32::from(ratio >= 50),
                3 => i32::from(ratio >= 75),
                // case 0 and default — note there is NO case 4 here.
                _ => 0,
            };
        }
    }

    if ctrls.qp_opt {
        offset = divide_and_round(offset * q_weight as i32, q_weight_denom as i32);
    }
    offset
}

/// Which arm of `derive_tf_window_params` a picture takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TfWindowArm {
    /// `pred_structure != RANDOM_ACCESS` (`pd_process.c:3851-3921`).
    LowDelay,
    /// `svt_aom_is_delayed_intra(pcs)` (`:3922-3965`).
    DelayedIntra,
    /// `pcs->idr_flag` inside random access (`:3966-4002`).
    RandomAccessIdr,
    /// Everything else inside random access (`:4004-4100`).
    RandomAccessInter,
}

/// The past/future picture COUNTS `derive_tf_window_params` derives, before
/// the buffers are searched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TfWindowCounts {
    /// C `num_past_pics`.
    pub num_past_pics: i32,
    /// C `num_future_pics`.
    pub num_future_pics: i32,
}

/// C `derive_tf_window_params`' COUNT derivation, per arm
/// (`pd_process.c:3851`, `:3930`, `:3973`, `:4005-4015`) — static.
///
/// This is the half that decides how wide the filter window may be; the other
/// half searches the reorder queue / mini-GOP array for pictures to fill it,
/// which is buffer plumbing this port replaces.
///
/// The four arms differ in ways that are easy to blur together:
/// * LOW DELAY adds `offset` only when `modulate_pics` is set, and caps
///   against `max_num_{past,future}_pics` — no `tf_max_ref_per_struct` cap.
/// * DELAYED INTRA has NO past pictures at all and caps future against
///   `tf_max_ref_per_struct(hier, 0, 1)` — the I_SLICE row, `1 << hier`.
/// * RANDOM-ACCESS IDR is the same shape as delayed intra.
/// * RANDOM-ACCESS INTER takes `MAX(1, base + offset)` on BOTH sides — the
///   `MAX(1, ...)` floor exists only here, so a negative modulation cannot
///   empty the window — then caps against `max_num_*` and against
///   `tf_max_ref_per_struct(hier, temporal_layer ? 2 : 1, dir)`.
///
/// Note that the inter arm adds `offset` UNCONDITIONALLY, while the low-delay
/// arm adds it only under `modulate_pics`. `offset` is itself zero when
/// `modulate_pics` is 0 (the caller gates `ref_pics_modulation` on it), so the
/// two agree in practice — but only because of that outer gate.
#[must_use]
pub fn derive_tf_window_counts(
    arm: TfWindowArm,
    ctrls: &TfCtrls,
    offset: i32,
    hierarchical_levels: u32,
    temporal_layer_index: u8,
) -> TfWindowCounts {
    match arm {
        TfWindowArm::LowDelay => {
            let modulation = if ctrls.modulate_pics != 0 { offset } else { 0 };
            let num_past = (i32::from(ctrls.num_past_pics) + modulation)
                .min(i32::from(ctrls.max_num_past_pics));
            let num_future = (i32::from(ctrls.num_future_pics) + modulation)
                .min(i32::from(ctrls.max_num_future_pics));
            TfWindowCounts {
                num_past_pics: num_past,
                num_future_pics: num_future,
            }
        }
        TfWindowArm::DelayedIntra | TfWindowArm::RandomAccessIdr => {
            let modulation = if ctrls.modulate_pics != 0 { offset } else { 0 };
            // C computes this in a uint32_t, so a negative modulation wraps
            // before the MIN clamps it back down. Reproduced with a saturating
            // cast through u32 exactly as C does.
            let raw = (u32::from(ctrls.num_future_pics)).wrapping_add(modulation as u32);
            let num_future = raw.min(u32::from(ctrls.max_num_future_pics));
            let num_future = num_future.min(u32::from(tf_max_ref_per_struct(
                hierarchical_levels,
                0,
                true,
            )));
            TfWindowCounts {
                num_past_pics: 0,
                num_future_pics: num_future as i32,
            }
        }
        TfWindowArm::RandomAccessInter => {
            // The MAX(1, ...) floor exists ONLY on this arm.
            let mut num_past = 1.max(i32::from(ctrls.num_past_pics) + offset);
            let mut num_future = 1.max(i32::from(ctrls.num_future_pics) + offset);
            num_past = num_past.min(i32::from(ctrls.max_num_past_pics));
            num_future = num_future.min(i32::from(ctrls.max_num_future_pics));
            let ty = if temporal_layer_index != 0 { 2 } else { 1 };
            num_past = num_past.min(i32::from(tf_max_ref_per_struct(
                hierarchical_levels,
                ty,
                false,
            )));
            num_future = num_future.min(i32::from(tf_max_ref_per_struct(
                hierarchical_levels,
                ty,
                true,
            )));
            TfWindowCounts {
                num_past_pics: num_past,
                num_future_pics: num_future,
            }
        }
    }
}

/// C's past-window compaction (`pd_process.c:3914-3920` and `:4091-4098`) —
/// static.
///
/// When fewer past pictures were found than requested, the list is shifted
/// LEFT by the shortfall so the centre lands at index `actual_past_pics`.
///
/// **This block is UNREACHABLE in C** and is translated anyway, per
/// `docs/WORKING-ON-THIS.md` §7. `actual_past_pics` is initialised to
/// `num_past_pics` at `:3873` and `:4042` and never modified — only
/// `actual_future_pics` is incremented — so `actual_past_pics != num_past_pics`
/// is always false. Written up as `docs/SUSPECTED-C-BUGS.md` #18, with the
/// second-order finding that even a fixed counter would not fix the block:
/// C's loop is `while (list[pic_i] != NULL)` from index 0, and the situation
/// the block was written for is exactly the one that puts a NULL at index 0.
///
/// The caller must reproduce C's `actual_past_pics == num_past_pics` and
/// therefore never invoke this.
pub fn compact_tf_past_window(
    list: &mut [Option<usize>; ALTREF_MAX_NFRAMES],
    num_past_pics: usize,
    actual_past_pics: usize,
) {
    if actual_past_pics == num_past_pics {
        return;
    }
    let shift = num_past_pics - actual_past_pics;
    let mut i = 0usize;
    while i < ALTREF_MAX_NFRAMES && list[i].is_some() {
        list[i] = if i + shift < ALTREF_MAX_NFRAMES {
            list[i + shift]
        } else {
            None
        };
        i += 1;
    }
}

/// C's `tf_avg_luma` / `tf_avg_ahd_error` reduction
/// (`pd_process.c:4101-4118`) — static.
///
/// Averages the window's luma means and AHD errors, EXCLUDING the centre
/// picture (the one at index `past_altref_nframes`).
///
/// Returns `(tf_avg_luma, tf_avg_ahd_error)`, both zero when the window is
/// empty — C leaves `tf_avg_ahd_error` at 0 and does not touch `tf_avg_luma`
/// in that case, which this reproduces by returning the zero pair.
#[must_use]
pub fn tf_window_averages(
    window_avg_luma: &[u64],
    window_ahd_error: &[i32],
    past_altref_nframes: usize,
    future_altref_nframes: usize,
) -> (u64, i32) {
    let n = past_altref_nframes + future_altref_nframes;
    if n == 0 {
        return (0, 0);
    }
    let mut tot_luma: u64 = 0;
    let mut tot_err: i32 = 0;
    for i in 0..=n {
        if i != past_altref_nframes {
            tot_luma = tot_luma.wrapping_add(window_avg_luma[i]);
            tot_err = tot_err.wrapping_add(window_ahd_error[i]);
        }
    }
    (tot_luma / n as u64, tot_err / n as i32)
}

// ---------------------------------------------------------------------------
// `derive_tf_window_params`' list half: candidates, assembly, noise carry
// ---------------------------------------------------------------------------

/// One picture the TF-window search may pick — the per-pcs fields C reads
/// off each `PictureParentControlSet*` while filling `temp_filt_pcs_list`
/// (`pd_process.c:3850-4118`).
///
/// C reaches candidates through three structures — `mg_pictures_array`, the
/// picture-decision reorder queue, and the pre-assignment buffer. Under this
/// port's one-buffer-at-a-time RA driver all three resolve to entries of the
/// same display-ordered slice, so the caller passes one `cands` view and the
/// bounds of the centre's own mini-GOP inside it (the only sub-range C
/// distinguishes — the past side of the inter arm and the whole
/// delayed-intra future search are bounded to `mg_pictures_array`).
#[derive(Debug, Clone, Copy)]
pub struct TfWindowCand<'a> {
    /// C `pcs->picture_number`.
    pub picture_number: u64,
    /// C `pcs->frame_width` — the UNALIGNED source width; a candidate whose
    /// resolution differs from the centre's is excluded.
    pub frame_width: u32,
    /// C `pcs->frame_height`.
    pub frame_height: u32,
    /// C `pcs->hierarchical_levels` — read only by the delayed-intra
    /// pred-structure fixup (`pd_process.c:3936-3943`).
    pub hierarchical_levels: u8,
    /// C `pcs->avg_luma` — `INVALID_LUMA` when no statistics were gathered.
    pub avg_luma: u64,
    /// C `pcs->picture_histogram` — the per-region luma histograms `calc_ahd`
    /// sums absolute bin differences over.
    pub picture_histogram: &'a RegionHistograms,
}

/// Which caller-supplied candidate slice a [`TfWindowMember`] indexes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TfMemberPool {
    /// The buffered random-access window — C's `mg_pictures_array`, reorder
    /// queue and pre-assignment buffer, unified here.
    Window,
    /// C `pd_ctx->tf_pic_array` — the low-delay ring
    /// `low_delay_store_tf_pictures` fills. Only the low-delay arm produces
    /// members in this pool.
    LdRing,
}

/// One filled `temp_filt_pcs_list` slot — the member's own locator plus the
/// fields C stamps on the member's pcs while assembling the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TfWindowMember {
    /// Index into the slice [`TfMemberPool`] names.
    pub index: usize,
    /// Which candidate slice `index` addresses.
    pub pool: TfMemberPool,
    /// C `pcs->avg_luma` — carried so `tf_avg_luma` needs no second lookup.
    pub avg_luma: u64,
    /// C `pcs->tf_ahd_error_to_central` — `calc_ahd` between centre and
    /// member. `0` on arms C never computes it (low delay) and on the centre
    /// slot of the delayed-intra/IDR arms.
    pub ahd_error_to_central: u32,
    /// C `pcs->tf_active_region_present` — `active_region_cnt > 0`.
    pub active_region_present: bool,
}

/// The assembled `temp_filt_pcs_list` for one centre picture
/// (`pd_process.c:3850-4118`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TfWindow {
    /// Slot-addressed exactly like C's `pcs->temp_filt_pcs_list`: slots
    /// `0..past_altref_nframes` are past members, `past_altref_nframes` is
    /// the centre, `past+1..past+1+future` are future members. `None` is C's
    /// `NULL` — a slot `search_this_pic` failed to fill (only the sparse
    /// fills can produce it; C counts those slots in `past_altref_nframes`
    /// regardless).
    pub members: alloc::vec::Vec<Option<TfWindowMember>>,
    /// C `pcs->past_altref_nframes` — the REQUESTED (clamped) past count, not
    /// the count actually found: C never adjusts it (`actual_past_pics` is
    /// assigned `num_past_pics`, never incremented).
    pub past_altref_nframes: usize,
    /// C `pcs->future_altref_nframes` — the count actually found.
    pub future_altref_nframes: usize,
    /// C `pcs->tf_avg_luma` — member `avg_luma` mean, centre excluded.
    pub tf_avg_luma: u64,
    /// C `pcs->tf_avg_ahd_error`.
    pub tf_avg_ahd_error: i32,
    /// The delayed-intra `hierarchical_levels` fixup (`pd_process.c:3936-3943`):
    /// when poc+1 exists in the mini-GOP and its level differs from the
    /// centre's, C stamps the MEMBER's level onto the centre (via
    /// `temp_filt_pcs_list[0]`, which IS the centre) and writes it back —
    /// `Some(level)` tells the caller to stamp `level` on the centre's
    /// `hierarchical_levels` (the member's level is already the source value).
    pub hier_fixup: Option<u8>,
}

/// C `derive_tf_window_params`' LIST half (`pd_process.c:3850-4118`).
///
/// Fills the `temp_filt_pcs_list` equivalent for one centre picture and
/// stamps the window statistics. The C structures collapse like this:
///
/// * **past, low-delay arm** (`:3863-3870`): `search_this_pic` over
///   `pd_ctx->tf_pic_array` → `ld_past`, NO dims check, NO `calc_ahd`.
/// * **past+centre, inter arm** (`:4030-4041`): `search_this_pic` over
///   `mg_pictures_array` → `cands[mg_lo..mg_hi]`, dims-checked with `break`,
///   `calc_ahd` stamped on each found member — INCLUDING the centre itself
///   (whose histogram difference against itself is trivially 0).
/// * **future, delayed-intra arm** (`:3946-3961`): `search_this_pic` over
///   `mg_pictures_array`, dims-checked with `break`, `calc_ahd` stamped.
/// * **future, IDR arm** (`:3977-3995`): the reorder-queue walk only — the
///   (i+1)-th entry after the centre in input order, dims-checked, `calc_ahd`
///   stamped. Input order is display order here, so the walk is positional:
///   `cands[centre_idx + 1 + i]`.
/// * **future, inter arm** (`:4051-4087`): the same positional queue walk,
///   then `search_this_pic` over the pre-assignment buffer for the remaining
///   slots. Contiguous POCs make the two identical — both resolve to the
///   positional walk.
/// * **past/future, low-delay arm**: same queue walk for the future side.
///
/// `avail_past` is the caller's `avail_past_pictures(mg_pocs, centre_poc)` —
/// the inter arm's same-mini-GOP bound (`pd_process.c:4024-4028`).
///
/// The compaction block C gates on `actual_past_pics != num_past_pics` is
/// dead in C (the two are always equal — `compact_tf_past_window` documents
/// it), so `past_altref_nframes` is the REQUESTED count even when some past
/// slots are `None`.
#[must_use]
pub fn assemble_tf_window(
    arm: TfWindowArm,
    counts: &TfWindowCounts,
    centre_idx: usize,
    cands: &[TfWindowCand],
    mg_lo: usize,
    mg_hi: usize,
    ld_past: &[TfWindowCand],
    avail_past: i32,
    regions_per_width: usize,
    regions_per_height: usize,
) -> TfWindow {
    let centre = &cands[centre_idx];
    let (cw, ch) = (centre.frame_width, centre.frame_height);
    let poc = centre.picture_number;
    // `search_this_pic` works on POC slices; the two arrays C searches are
    // collected once here.
    let mg_pocs: alloc::vec::Vec<u64> = cands[mg_lo..mg_hi]
        .iter()
        .map(|c| c.picture_number)
        .collect();
    let ld_pocs: alloc::vec::Vec<u64> = ld_past.iter().map(|c| c.picture_number).collect();
    let mut members: alloc::vec::Vec<Option<TfWindowMember>> =
        alloc::vec![None; ALTREF_MAX_NFRAMES];

    let member = |index: usize, pool: TfMemberPool, c: &TfWindowCand, do_ahd: bool| {
        let (ahd, active) = if do_ahd {
            calc_ahd(
                centre.picture_histogram,
                c.picture_histogram,
                c.frame_width,
                c.frame_height,
                regions_per_width,
                regions_per_height,
            )
        } else {
            (0, 0)
        };
        TfWindowMember {
            index,
            pool,
            avg_luma: c.avg_luma,
            ahd_error_to_central: ahd,
            active_region_present: active > 0,
        }
    };

    let num_past = counts.num_past_pics.max(0) as usize;
    let num_future = counts.num_future_pics.max(0) as usize;
    let mut past = 0usize;
    let mut future = 0usize;
    let mut hier_fixup = None;

    match arm {
        TfWindowArm::LowDelay => {
            // `pd_process.c:3863-3880`: past from `tf_pic_array` — no dims
            // check, no AHD; a miss leaves the slot NULL but counts anyway.
            for pic_itr in 0..num_past {
                let target = poc
                    .wrapping_sub(num_past as u64)
                    .wrapping_add(pic_itr as u64);
                let i = search_this_pic(&ld_pocs, target);
                if i >= 0 {
                    let i = i as usize;
                    members[pic_itr] = Some(member(i, TfMemberPool::LdRing, &ld_past[i], false));
                }
            }
            // `:3882` — centre at slot `num_past`.
            members[num_past] = Some(member(centre_idx, TfMemberPool::Window, centre, false));
            // `:3886-3912` — future: queue walk then pre-ass buffer; both are
            // the positional forward walk here. Dims mismatch breaks.
            for pic_i in 0..num_future {
                match cands.get(centre_idx + 1 + pic_i) {
                    Some(c) if c.frame_width == cw && c.frame_height == ch => {
                        members[num_past + 1 + pic_i] = Some(member(
                            centre_idx + 1 + pic_i,
                            TfMemberPool::Window,
                            c,
                            false,
                        ));
                        future += 1;
                    }
                    _ => break,
                }
            }
            // `:3908-3909` — `actual_past_pics` is `num_past_pics` verbatim;
            // the compaction below it is dead (see `compact_tf_past_window`).
            past = num_past;
        }
        TfWindowArm::DelayedIntra => {
            // `:3932` — centre at slot 0; NO ahd stamped on it.
            members[0] = Some(member(centre_idx, TfMemberPool::Window, centre, false));
            // `:3936-3943` — the key-frame pred-structure fixup. The
            // `centre != temp_filt_pcs_list[0]` half is dead (slot 0 IS the
            // centre); only the member comparison can fire.
            let next = poc.wrapping_add(1);
            let fixup_idx = search_this_pic(&mg_pocs, next);
            if fixup_idx >= 0 {
                let lvl = cands[mg_lo + fixup_idx as usize].hierarchical_levels;
                if lvl != centre.hierarchical_levels {
                    hier_fixup = Some(lvl);
                }
            }
            // `:3946-3961` — future from `mg_pictures_array` only, poc-matched.
            for pic_i in 0..num_future {
                let target = poc.wrapping_add(pic_i as u64 + 1);
                let local = search_this_pic(&mg_pocs, target);
                if local < 0 {
                    break;
                }
                let idx = mg_lo + local as usize;
                let c = &cands[idx];
                if c.frame_width != cw || c.frame_height != ch {
                    break;
                }
                members[pic_i + 1] = Some(member(idx, TfMemberPool::Window, c, true));
                future += 1;
            }
        }
        TfWindowArm::RandomAccessIdr => {
            // `:3971` — centre at slot 0; NO ahd stamped.
            members[0] = Some(member(centre_idx, TfMemberPool::Window, centre, false));
            // `:3977-3995` — future from the reorder queue only (positional).
            for pic_i in 0..num_future {
                match cands.get(centre_idx + 1 + pic_i) {
                    Some(c) if c.frame_width == cw && c.frame_height == ch => {
                        members[pic_i + 1] = Some(member(
                            centre_idx + 1 + pic_i,
                            TfMemberPool::Window,
                            c,
                            true,
                        ));
                        future += 1;
                    }
                    _ => break,
                }
            }
        }
        TfWindowArm::RandomAccessInter => {
            // `:4024-4028` — bound to what the mini-GOP actually holds.
            let num_past = num_past.min(avail_past.max(0) as usize);
            // `:4030-4041` — past+centre from `mg_pictures_array`, poc-matched;
            // dims mismatch breaks, a plain miss leaves the slot NULL.
            for pic_itr in 0..=num_past {
                let target = poc
                    .wrapping_sub(num_past as u64)
                    .wrapping_add(pic_itr as u64);
                let local = search_this_pic(&mg_pocs, target);
                if local >= 0 {
                    let idx = mg_lo + local as usize;
                    let c = &cands[idx];
                    if c.frame_width != cw || c.frame_height != ch {
                        break;
                    }
                    members[pic_itr] = Some(member(idx, TfMemberPool::Window, c, true));
                }
            }
            // `:4051-4087` — queue walk then pre-ass buffer: the positional
            // forward walk.
            for pic_i in 0..num_future {
                match cands.get(centre_idx + 1 + pic_i) {
                    Some(c) if c.frame_width == cw && c.frame_height == ch => {
                        members[num_past + 1 + pic_i] = Some(member(
                            centre_idx + 1 + pic_i,
                            TfMemberPool::Window,
                            c,
                            true,
                        ));
                        future += 1;
                    }
                    _ => break,
                }
            }
            // `:4089` — `actual_past_pics` is `num_past_pics` verbatim.
            past = num_past;
        }
    }

    // `:4101-4118` — averages over slots `0..=past+future`, centre excluded.
    members.truncate(past + 1 + future);
    let mut luma = alloc::vec::Vec::with_capacity(members.len());
    let mut errs = alloc::vec::Vec::with_capacity(members.len());
    for m in &members {
        // C dereferences `temp_filt_pcs_list[i]` unconditionally — a NULL
        // slot would crash there; the port treats it as a 0 contribution,
        // which is only reachable on the sparse fills.
        let (l, e) = m.map_or((0, 0), |m| (m.avg_luma, m.ahd_error_to_central as i32));
        luma.push(l);
        errs.push(e);
    }
    let (tf_avg_luma, tf_avg_ahd_error) = tf_window_averages(&luma, &errs, past, future);

    TfWindow {
        members,
        past_altref_nframes: past,
        future_altref_nframes: future,
        tf_avg_luma,
        tf_avg_ahd_error,
        hier_fixup,
    }
}

/// The noise state `derive_tf_window_params` stamps on the centre
/// (`pd_process.c:3755-3849`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TfWindowNoise {
    /// C `pcs->noise_levels_log1p_fp16[3]` — the Q16 `log1p` levels per plane.
    pub levels_log1p_fp16: [i32; 3],
    /// C `pcs->is_noise_level` — read off `last_i` AFTER the carry, identical
    /// to reading `levels[0]`.
    pub is_noise_level: bool,
    /// C `do_noise_est` — whether the fresh Y estimate ran. When false,
    /// `levels[0]` is the carried `last_i` value.
    pub estimated_y: bool,
}

/// C `derive_tf_window_params`' NOISE half (`pd_process.c:3755-3849`).
///
/// The pixel estimation itself belongs to the caller (the ported
/// `svt_estimate_noise*_fp16` kernels); this owns the SELECTION:
///
/// * `do_noise_est = !use_intra_for_noise_est || slice_type == I_SLICE` — an
///   I slice ALWAYS re-estimates whatever the control table says.
/// * Y is estimated only under `do_noise_est`; U/V are estimated
///   UNCONDITIONALLY when `chroma_lvl` is set — the last-I carry applies to
///   luma alone.
/// * `last_i[0]` is updated from the fresh estimate, or reloaded into
///   `levels[0]` when the estimate was skipped.
/// * `is_noise_level = last_i[0] >= VQ_NOISE_LVL_TH`, read after the carry.
///
/// `fresh_y_log1p_fp16` must be `Some` exactly when `do_noise_est` holds —
/// the caller computes it with `estimate_noise_fp16` / `_highbd_fp16` over the
/// centre's source and runs it through `noise_log1p_fp16`. `fresh_uv` carries
/// the same for chroma, required only when `chroma_lvl` is set; when it is
/// not set C leaves the pcs's zero-initialised U/V slots alone, which the
/// `[0, 0]` stamp reproduces for a picture this runs once on.
#[must_use]
pub fn tf_window_noise(
    use_intra_for_noise_est: bool,
    is_i_slice: bool,
    chroma_lvl: bool,
    fresh_y_log1p_fp16: Option<i32>,
    fresh_uv_log1p_fp16: [i32; 2],
    last_i_noise_levels_log1p_fp16: &mut [i32; 3],
) -> TfWindowNoise {
    // `pd_process.c:3757-3759`.
    let do_noise_est = !use_intra_for_noise_est || is_i_slice;
    let y = if do_noise_est {
        let y = fresh_y_log1p_fp16
            .expect("do_noise_est is set: C estimates Y — the caller must supply it");
        // `:3844` — publish to the carry slot.
        last_i_noise_levels_log1p_fp16[0] = y;
        y
    } else {
        debug_assert!(
            fresh_y_log1p_fp16.is_none(),
            "do_noise_est is clear: C does not estimate Y, but a fresh value was supplied"
        );
        // `:3846` — reuse the carried I-slice noise.
        last_i_noise_levels_log1p_fp16[0]
    };
    // `:3794-3810` / `:3826-3841` — chroma estimates are UNCONDITIONAL under
    // `chroma_lvl`; with the gate off C keeps the pcs's initial zeros.
    let [u, v] = if chroma_lvl {
        fresh_uv_log1p_fp16
    } else {
        [0, 0]
    };
    TfWindowNoise {
        levels_log1p_fp16: [y, u, v],
        // `:3848` — reads `last_i` after the carry; equal to `y` in force.
        is_noise_level: last_i_noise_levels_log1p_fp16[0] >= VQ_NOISE_LVL_TH,
        estimated_y: do_noise_est,
    }
}

/// `pd_process.c:5122` — the centre inherits the context's carried
/// filt/unfilt difference before its window params derive (and before the
/// filter overwrites it with its own measurement).
pub fn tf_inherit_filt_to_unfilt_diff(ctx: &PicDecisionCtx, pic: &mut PicParams) {
    pic.filt_to_unfilt_diff = ctx.filt_to_unfilt_diff;
}

/// `pd_process.c:5124` — only an I slice publishes its measured
/// filt/unfilt difference back to the context.
pub fn tf_publish_filt_to_unfilt_diff(ctx: &mut PicDecisionCtx, pic: &PicParams) {
    if pic.slice_type == SliceType::I {
        ctx.filt_to_unfilt_diff = pic.filt_to_unfilt_diff;
    }
}

/// C `low_delay_store_tf_pictures`' STORE PREDICATE
/// (`pd_process.c:4127-4147`) — static.
///
/// A non-base low-delay picture joins the ring only when it is close enough to
/// the end of the mini-GOP to be a past reference for the upcoming base:
/// `temporal_layer_index != 0 && pic_idx_in_mg + 1 + tot_past >= mg_size`.
///
/// Reachability: in current mainline low-delay TF is disabled
/// (`enc_handle.c:3339-3343`), so this is dead for the first cell. It becomes
/// live the moment `tf_ld_controls` is given a non-zero level, which is why it
/// is translated (`docs/WORKING-ON-THIS.md` §7). The live-count bookkeeping
/// around it is buffer plumbing this port replaces.
#[must_use]
pub fn low_delay_should_store_tf_picture(
    temporal_layer_index: u8,
    pic_idx_in_mg: u32,
    max_num_past_pics: u8,
    hierarchical_levels: u32,
) -> bool {
    let mg_size = 1u32 << hierarchical_levels;
    temporal_layer_index != 0 && pic_idx_in_mg + 1 + u32::from(max_num_past_pics) >= mg_size
}

/// C `mctf_frame`'s decision half (`pd_process.c:4194-4250`) — static.
///
/// Everything in `mctf_frame` that is not fifo posting or semaphore waiting:
/// which of the two low-delay ring operations run, whether TF runs at all,
/// the motion-direction verdict and `is_noise_level`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MctfFrameDecision {
    /// Run `low_delay_store_tf_pictures` before filtering.
    pub store_ld_tf_pictures: bool,
    /// Run `derive_tf_window_params` + the filter.
    pub run_tf: bool,
    /// C `pcs->do_tf` — set FALSE when TF is off; C never sets it true here.
    pub do_tf_cleared: bool,
    /// C `pcs->is_noise_level`.
    pub is_noise_level: bool,
    /// Release the low-delay ring after filtering.
    pub release_ld_tf_pictures: bool,
}

/// C `mctf_frame` (`pd_process.c:4194-4250`) — static, decision half.
///
/// Trap: the STORE gate and the RELEASE gate are NOT symmetric. Both require
/// `pred_structure != RANDOM_ACCESS && tf_params_per_type[1].enabled`, but the
/// RELEASE additionally requires `temporal_layer_index == 0` — the ring is
/// filled by non-base pictures and drained by the base picture that consumed
/// it.
#[must_use]
pub fn mctf_frame_decision(
    seq_pred_structure: PredStructure,
    base_tf_params_enabled: bool,
    tf_ctrls_enabled: bool,
    temporal_layer_index: u8,
    last_i_noise_levels_log1p_fp16: i32,
) -> MctfFrameDecision {
    let ld = seq_pred_structure != PredStructure::RandomAccess;
    MctfFrameDecision {
        store_ld_tf_pictures: ld && base_tf_params_enabled,
        run_tf: tf_ctrls_enabled,
        do_tf_cleared: !tf_ctrls_enabled,
        is_noise_level: last_i_noise_levels_log1p_fp16 >= VQ_NOISE_LVL_TH,
        release_ld_tf_pictures: ld && base_tf_params_enabled && temporal_layer_index == 0,
    }
}

/// C `mctf_frame`'s motion-direction verdict (`pd_process.c:4232-4238`).
///
/// `0` horizontal, `1` vertical, `-1` neither. The comparison is
/// `horz > vert * 6 / 4`, i.e. a 1.5x margin, evaluated with INTEGER division
/// on the right-hand side — `vert * 6 / 4` truncates, so at `vert == 1` the
/// threshold is 1, not 1.5.
#[must_use]
pub fn tf_motion_direction(tf_tot_horz_blks: u32, tf_tot_vert_blks: u32) -> i8 {
    if tf_tot_horz_blks > tf_tot_vert_blks * 6 / 4 {
        0
    } else if tf_tot_vert_blks > tf_tot_horz_blks * 6 / 4 {
        1
    } else {
        -1
    }
}

/// One step of C `mctf_frame_st` (`pd_process.c:4175-4193`) — static.
///
/// The single-threaded temporal-filter dispatch. It is small, but it is where
/// the TF call sits in the per-frame sequence, and the ORDER is the content:
/// every step reads state a previous one wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MctfStStep {
    /// `me_ctx->me_type = ME_MCTF` — must precede the signal derivation, which
    /// branches on it.
    SetMeTypeMctf,
    /// `svt_aom_sig_deriv_me_tf(pcs, me_ctx)`.
    SigDerivMeTf,
    /// `svt_aom_gm_pre_processor(pcs, pcs->temp_filt_pcs_list)` — CONDITIONAL
    /// on `pcs->gm_ctrls.pp_enabled && pcs->gm_pp_enabled`.
    GmPreProcessor,
    /// `svt_av1_init_temporal_filtering(...)` once per segment, in index order.
    InitTemporalFilteringSegment(u16),
    /// Consume the semaphore the last segment posted.
    ConsumeDoneSemaphore,
}

/// C `mctf_frame_st` (`pd_process.c:4175-4193`) — static.
///
/// Returns the exact step sequence for `tf_segments_total_count` segments.
/// The callees themselves live in other modules (ME signal derivation, global
/// motion, temporal filtering), so what this port owns is the ORDER — which is
/// the part a reimplementation gets wrong, because `me_type` must be set
/// before the signal derivation reads it and the global-motion pre-pass must
/// run before the first segment.
#[must_use]
pub fn mctf_frame_st_sequence(
    tf_segments_total_count: u16,
    gm_pp_enabled: bool,
) -> alloc::vec::Vec<MctfStStep> {
    let mut steps = alloc::vec::Vec::new();
    steps.push(MctfStStep::SetMeTypeMctf);
    steps.push(MctfStStep::SigDerivMeTf);
    if gm_pp_enabled {
        steps.push(MctfStStep::GmPreProcessor);
    }
    for seg in 0..tf_segments_total_count {
        steps.push(MctfStStep::InitTemporalFilteringSegment(seg));
    }
    steps.push(MctfStStep::ConsumeDoneSemaphore);
    steps
}

/// The low-delay temporal-filter picture ring
/// (`ctx->tf_pic_array` + `ctx->tf_pic_arr_cnt`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LowDelayTfRing {
    /// Picture identifiers in store order; C holds PPCS pointers.
    pub pics: alloc::vec::Vec<u64>,
}

/// C `low_delay_store_tf_pictures`' ring append
/// (`pd_process.c:4127-4147`) — static.
///
/// Appends only when [`low_delay_should_store_tf_picture`] says so. C also
/// increments the live count of five wrappers here (`p_pcs_wrapper_ptr`,
/// `input_pic_wrapper`, `pa_ref_pic_wrapper`, `scs_wrapper` and, when present,
/// `y8b_wrapper`) so the resources survive until TF consumes them; that is
/// buffer plumbing this port replaces, and it is named rather than dropped.
pub fn low_delay_store_tf_picture(
    ring: &mut LowDelayTfRing,
    picture_number: u64,
    temporal_layer_index: u8,
    pic_idx_in_mg: u32,
    max_num_past_pics: u8,
    hierarchical_levels: u32,
) {
    if low_delay_should_store_tf_picture(
        temporal_layer_index,
        pic_idx_in_mg,
        max_num_past_pics,
        hierarchical_levels,
    ) {
        ring.pics.push(picture_number);
    }
}

/// C `low_delay_release_tf_pictures` (`pd_process.c:4151-4174`) — static.
///
/// Drains the ring: C releases each stored picture's wrappers and then
/// `memset`s the array and zeroes the count.
///
/// The RELEASE ORDER is load-bearing in C and is recorded here even though
/// this port has no wrappers to release: `input_pic_wrapper`, then
/// `y8b_wrapper` if present, then `pa_ref_pic_wrapper`, then `scs_wrapper`,
/// and the PPCS **last** — the comment says so explicitly, because the PPCS
/// owns the handles the earlier releases read.
///
/// Note also that C `memset`s only `tf_pic_arr_cnt` entries, not the whole
/// array, so entries past the count keep stale pointers. Harmless because the
/// count gates every read, and reproduced here by clearing the whole vector
/// (which cannot expose a stale entry at all).
pub fn low_delay_release_tf_pictures(ring: &mut LowDelayTfRing) {
    ring.pics.clear();
}
