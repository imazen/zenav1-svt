//! Global motion in the INTER FRAME HEADER, plus the `aom_wb_*` bit-buffer
//! primitive stack it rides on, plus `write_sgrproj_filter`.
//!
//! C reference: `Source/Lib/Codec/entropy_coding.c`
//! (`aom_wb_write_primitive_quniform` :2882,
//! `aom_wb_write_primitive_subexpfin` :2929,
//! `aom_wb_write_primitive_refsubexpfin` :2984,
//! `svt_aom_wb_write_signed_primitive_refsubexpfin` :2989,
//! `write_global_motion_params` :3001, `write_global_motion` :3069,
//! `write_sgrproj_filter` :4069).
//!
//! # The bit-buffer variants are NOT the arithmetic-coder ones
//!
//! `entropy/lr.rs` already ports `svt_aom_write_primitive_refsubexpfin` /
//! `_subexpfin` / `_quniform`, but those write through `aom_write_bit` /
//! `aom_write_literal` into the ARITHMETIC coder. The three functions here
//! are C's separate `aom_wb_*` copies, which write through
//! `svt_aom_wb_write_bit` / `svt_aom_wb_write_literal` into the
//! UNCOMPRESSED-header bit buffer. Reusing the `lr.rs` pair would put global
//! motion into the wrong sink; the two stacks share only their arithmetic.

use crate::entropy::obu::BitWriter;
use crate::entropy::writer::AomWriter;

/// C `SUBEXPFIN_K` (definitions.h:1736).
pub const SUBEXPFIN_K: u16 = 3;
/// C `WARPEDMODEL_PREC_BITS` (definitions.h:1707).
pub const WARPEDMODEL_PREC_BITS: u32 = 16;
/// C `GM_TRANS_PREC_BITS` (definitions.h:1737).
pub const GM_TRANS_PREC_BITS: u32 = 6;
/// C `GM_ABS_TRANS_BITS` (definitions.h:1738).
pub const GM_ABS_TRANS_BITS: i32 = 12;
/// C `GM_ABS_TRANS_ONLY_BITS` (definitions.h:1739).
pub const GM_ABS_TRANS_ONLY_BITS: i32 = GM_ABS_TRANS_BITS - GM_TRANS_PREC_BITS as i32 + 3;
/// C `GM_TRANS_PREC_DIFF` (definitions.h:1740).
pub const GM_TRANS_PREC_DIFF: u32 = WARPEDMODEL_PREC_BITS - GM_TRANS_PREC_BITS;
/// C `GM_TRANS_ONLY_PREC_DIFF` (definitions.h:1741).
pub const GM_TRANS_ONLY_PREC_DIFF: u32 = WARPEDMODEL_PREC_BITS - 3;
/// C `GM_ALPHA_PREC_BITS` (definitions.h:1744).
pub const GM_ALPHA_PREC_BITS: u32 = 15;
/// C `GM_ABS_ALPHA_BITS` (definitions.h:1745).
pub const GM_ABS_ALPHA_BITS: u32 = 12;
/// C `GM_ALPHA_PREC_DIFF` (definitions.h:1746).
pub const GM_ALPHA_PREC_DIFF: u32 = WARPEDMODEL_PREC_BITS - GM_ALPHA_PREC_BITS;
/// C `GM_ALPHA_MAX` (definitions.h:1750).
pub const GM_ALPHA_MAX: u16 = 1 << GM_ABS_ALPHA_BITS;

/// C `recenter_nonneg` (entropy_coding.c:2845).
#[inline]
fn recenter_nonneg(r: u16, v: u16) -> u16 {
    if v > (r << 1) {
        v
    } else if v >= r {
        (v - r) << 1
    } else {
        ((r - v) << 1) - 1
    }
}

/// C `recenter_finite_nonneg` (entropy_coding.c:2859).
#[inline]
fn recenter_finite_nonneg(n: u16, r: u16, v: u16) -> u16 {
    if (r << 1) <= n {
        recenter_nonneg(r, v)
    } else {
        recenter_nonneg(n - 1 - r, n - 1 - v)
    }
}

/// C `get_msb` — index of the highest set bit; `n` must be nonzero.
#[inline]
fn get_msb(n: u32) -> u32 {
    debug_assert!(n != 0);
    31 - n.leading_zeros()
}

/// C `aom_wb_write_primitive_quniform` (entropy_coding.c:2882) — the
/// bit-buffer twin of `svt_aom_write_primitive_quniform`.
pub fn wb_write_primitive_quniform(wb: &mut BitWriter, n: u16, v: u16) {
    if n <= 1 {
        return;
    }
    let l = get_msb((n - 1) as u32) as i32 + 1;
    let m = (1i32 << l) - n as i32;
    if (v as i32) < m {
        wb.write_bits(v as u32, (l - 1) as u32);
    } else {
        wb.write_bits((m + ((v as i32 - m) >> 1)) as u32, (l - 1) as u32);
        wb.write_bit(((v as i32 - m) & 1) != 0);
    }
}

/// C `aom_wb_write_primitive_subexpfin` (entropy_coding.c:2929) — the
/// bit-buffer twin of `svt_aom_write_primitive_subexpfin`.
pub fn wb_write_primitive_subexpfin(wb: &mut BitWriter, n: u16, k: u16, v: u16) {
    let mut i = 0i32;
    let mut mk = 0i32;
    loop {
        let b = if i != 0 { k as i32 + i - 1 } else { k as i32 };
        let a = 1i32 << b;
        if (n as i32) <= mk + 3 * a {
            wb_write_primitive_quniform(wb, (n as i32 - mk) as u16, (v as i32 - mk) as u16);
            break;
        } else {
            let t = (v as i32) >= mk + a;
            wb.write_bit(t);
            if t {
                i += 1;
                mk += a;
            } else {
                wb.write_bits((v as i32 - mk) as u32, b as u32);
                break;
            }
        }
    }
}

/// C `aom_wb_write_primitive_refsubexpfin` (entropy_coding.c:2984).
pub fn wb_write_primitive_refsubexpfin(wb: &mut BitWriter, n: u16, k: u16, r: u16, v: u16) {
    wb_write_primitive_subexpfin(wb, n, k, recenter_finite_nonneg(n, r, v));
}

/// C `svt_aom_wb_write_signed_primitive_refsubexpfin` (entropy_coding.c:2989)
/// — the entry point every global-motion coefficient goes through.
pub fn wb_write_signed_primitive_refsubexpfin(wb: &mut BitWriter, n: u16, k: u16, r: i16, v: i16) {
    let r = r.wrapping_add(n as i16).wrapping_sub(1);
    let v = v.wrapping_add(n as i16).wrapping_sub(1);
    let scaled_n = (n << 1).wrapping_sub(1);
    wb_write_primitive_refsubexpfin(wb, scaled_n, k, r as u16, v as u16);
}

/// C `WarpedMotionParams`, cut down to the fields the header writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WarpParams {
    /// C `wmtype`.
    pub wmtype: crate::port_entropy_inter::modes::TransformationType,
    /// C `wmmat[0..6]`.
    pub wmmat: [i32; 6],
}

impl WarpParams {
    /// C `default_warp_params` (definitions.h:1789) — IDENTITY with the two
    /// diagonal entries at `1 << WARPEDMODEL_PREC_BITS`.
    pub const IDENTITY: Self = Self {
        wmtype: crate::port_entropy_inter::modes::TransformationType::Identity,
        wmmat: [
            0,
            0,
            1 << WARPEDMODEL_PREC_BITS,
            0,
            0,
            1 << WARPEDMODEL_PREC_BITS,
        ],
    };
}

/// C `write_global_motion_params` (entropy_coding.c:3001) — the per-reference
/// warp-parameter encoding: the type tree, then the ROTZOOM / AFFINE /
/// TRANSLATION coefficients, each recentred against `ref_params`.
pub fn write_global_motion_params(
    wb: &mut BitWriter,
    params: &WarpParams,
    ref_params: &WarpParams,
    allow_hp: bool,
) {
    use crate::port_entropy_inter::modes::TransformationType as T;
    let ty = params.wmtype;
    wb.write_bit(ty != T::Identity);
    if ty != T::Identity {
        wb.write_bit(ty == T::RotZoom);
        if ty != T::RotZoom {
            wb.write_bit(ty == T::Translation);
        }
    }

    if ty >= T::RotZoom {
        let ref2 = ((ref_params.wmmat[2] >> GM_ALPHA_PREC_DIFF) - (1 << GM_ALPHA_PREC_BITS)) as i16;
        let v2 = ((params.wmmat[2] >> GM_ALPHA_PREC_DIFF) - (1 << GM_ALPHA_PREC_BITS)) as i16;
        let ref3 = (ref_params.wmmat[3] >> GM_ALPHA_PREC_DIFF) as i16;
        let v3 = (params.wmmat[3] >> GM_ALPHA_PREC_DIFF) as i16;
        wb_write_signed_primitive_refsubexpfin(wb, GM_ALPHA_MAX + 1, SUBEXPFIN_K, ref2, v2);
        wb_write_signed_primitive_refsubexpfin(wb, GM_ALPHA_MAX + 1, SUBEXPFIN_K, ref3, v3);
    }

    if ty >= T::Affine {
        let ref4 = (ref_params.wmmat[4] >> GM_ALPHA_PREC_DIFF) as i16;
        let v4 = (params.wmmat[4] >> GM_ALPHA_PREC_DIFF) as i16;
        let ref5 = ((ref_params.wmmat[5] >> GM_ALPHA_PREC_DIFF) - (1 << GM_ALPHA_PREC_BITS)) as i16;
        let v5 = ((params.wmmat[5] >> GM_ALPHA_PREC_DIFF) - (1 << GM_ALPHA_PREC_BITS)) as i16;
        wb_write_signed_primitive_refsubexpfin(wb, GM_ALPHA_MAX + 1, SUBEXPFIN_K, ref4, v4);
        wb_write_signed_primitive_refsubexpfin(wb, GM_ALPHA_MAX + 1, SUBEXPFIN_K, ref5, v5);
    }

    if ty >= T::Translation {
        let trans_bits = if ty == T::Translation {
            GM_ABS_TRANS_ONLY_BITS - i32::from(!allow_hp)
        } else {
            GM_ABS_TRANS_BITS
        };
        let trans_prec_diff = if ty == T::Translation {
            GM_TRANS_ONLY_PREC_DIFF + u32::from(!allow_hp)
        } else {
            GM_TRANS_PREC_DIFF
        };
        let n = ((1i32 << trans_bits) + 1) as u16;
        wb_write_signed_primitive_refsubexpfin(
            wb,
            n,
            SUBEXPFIN_K,
            (ref_params.wmmat[0] >> trans_prec_diff) as i16,
            (params.wmmat[0] >> trans_prec_diff) as i16,
        );
        wb_write_signed_primitive_refsubexpfin(
            wb,
            n,
            SUBEXPFIN_K,
            (ref_params.wmmat[1] >> trans_prec_diff) as i16,
            (params.wmmat[1] >> trans_prec_diff) as i16,
        );
    }
}

/// C `PRIMARY_REF_NONE` (definitions.h).
pub const PRIMARY_REF_NONE: u8 = 7;

/// C `write_global_motion` (entropy_coding.c:3069) — the inter frame header's
/// LAST..ALTREF loop.
///
/// The reference each frame's params are coded AGAINST is
/// `ref_global_motion[frame]` when `primary_ref_frame != PRIMARY_REF_NONE`,
/// and `default_warp_params` (IDENTITY) otherwise. `entropy/obu.rs`'s current
/// inter header writes seven hardcoded zero bits, which is only accidentally
/// right for all-IDENTITY with no CDF continuation.
pub fn write_global_motion(
    wb: &mut BitWriter,
    global_motion: &[WarpParams; 8],
    ref_global_motion: &[WarpParams; 8],
    primary_ref_frame: u8,
    allow_high_precision_mv: bool,
) {
    for frame in 1usize..=7 {
        let ref_params = if primary_ref_frame != PRIMARY_REF_NONE {
            &ref_global_motion[frame]
        } else {
            &WarpParams::IDENTITY
        };
        write_global_motion_params(
            wb,
            &global_motion[frame],
            ref_params,
            allow_high_precision_mv,
        );
    }
}

// ---- write_sgrproj_filter (entropy_coding.c:4069) ----

/// C `SGRPROJ_PARAMS_BITS` (restoration.h:90).
pub const SGRPROJ_PARAMS_BITS: u32 = 4;
/// C `SGRPROJ_PRJ_BITS` (restoration.h:94).
pub const SGRPROJ_PRJ_BITS: i32 = 7;
/// C `SGRPROJ_PRJ_MIN0` (restoration.h:101).
pub const SGRPROJ_PRJ_MIN0: i32 = -(1 << SGRPROJ_PRJ_BITS) * 3 / 4;
/// C `SGRPROJ_PRJ_MAX0` (restoration.h:102).
pub const SGRPROJ_PRJ_MAX0: i32 = SGRPROJ_PRJ_MIN0 + (1 << SGRPROJ_PRJ_BITS) - 1;
/// C `SGRPROJ_PRJ_MIN1` (restoration.h:103).
pub const SGRPROJ_PRJ_MIN1: i32 = -(1 << SGRPROJ_PRJ_BITS) / 4;
/// C `SGRPROJ_PRJ_MAX1` (restoration.h:104).
pub const SGRPROJ_PRJ_MAX1: i32 = SGRPROJ_PRJ_MIN1 + (1 << SGRPROJ_PRJ_BITS) - 1;
/// C `SGRPROJ_PRJ_SUBEXP_K` (restoration.h:106).
pub const SGRPROJ_PRJ_SUBEXP_K: u16 = 4;

/// C `SgrprojInfo` (restoration.h).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SgrprojInfo {
    /// Self-guided parameter-set index, `0..16`.
    pub ep: u8,
    /// The two projection coefficients.
    pub xqd: [i32; 2],
}

/// C `write_sgrproj_filter` (entropy_coding.c:4069).
///
/// `r0_is_zero` / `r1_is_zero` are `svt_aom_eb_sgr_params[ep].r[0] == 0` /
/// `.r[1] == 0`; the parameter table lives in `restoration.rs`, which this
/// lane does not own, so the two flags are passed in rather than looked up.
///
/// Reachability, corrected in place: `rust/CLAUDE.md` envelope guard #5 says
/// SGR is dead for M0..M13 — that is the ALL-INTRA arm
/// (`svt_aom_get_sg_filter_level_allintra`, enc_mode_config.c:1431, called at
/// :2462). VIDEO mode takes `svt_aom_get_sg_filter_level_default`
/// (enc_mode_config.c:1402, called at :2134), which returns 3 for
/// `enc_mode <= ENC_M3`. So at `SVT_AVIF=0` and presets 0..3,
/// `RESTORE_SGRPROJ` / `RESTORE_SWITCHABLE` become emittable and this
/// function is LIVE. Guard #5 should read "all-intra M0..M13", not
/// unconditional.
pub fn write_sgrproj_filter(
    w: &mut AomWriter,
    info: &SgrprojInfo,
    ref_info: &mut SgrprojInfo,
    r0_is_zero: bool,
    r1_is_zero: bool,
) {
    use crate::entropy::lr::write_primitive_refsubexpfin;
    w.write_literal(info.ep as u32, SGRPROJ_PARAMS_BITS);

    if r0_is_zero {
        debug_assert_eq!(info.xqd[0], 0);
        write_primitive_refsubexpfin(
            w,
            (SGRPROJ_PRJ_MAX1 - SGRPROJ_PRJ_MIN1 + 1) as u16,
            SGRPROJ_PRJ_SUBEXP_K,
            (ref_info.xqd[1] - SGRPROJ_PRJ_MIN1) as u16,
            (info.xqd[1] - SGRPROJ_PRJ_MIN1) as u16,
        );
    } else if r1_is_zero {
        write_primitive_refsubexpfin(
            w,
            (SGRPROJ_PRJ_MAX0 - SGRPROJ_PRJ_MIN0 + 1) as u16,
            SGRPROJ_PRJ_SUBEXP_K,
            (ref_info.xqd[0] - SGRPROJ_PRJ_MIN0) as u16,
            (info.xqd[0] - SGRPROJ_PRJ_MIN0) as u16,
        );
    } else {
        write_primitive_refsubexpfin(
            w,
            (SGRPROJ_PRJ_MAX0 - SGRPROJ_PRJ_MIN0 + 1) as u16,
            SGRPROJ_PRJ_SUBEXP_K,
            (ref_info.xqd[0] - SGRPROJ_PRJ_MIN0) as u16,
            (info.xqd[0] - SGRPROJ_PRJ_MIN0) as u16,
        );
        write_primitive_refsubexpfin(
            w,
            (SGRPROJ_PRJ_MAX1 - SGRPROJ_PRJ_MIN1 + 1) as u16,
            SGRPROJ_PRJ_SUBEXP_K,
            (ref_info.xqd[1] - SGRPROJ_PRJ_MIN1) as u16,
            (info.xqd[1] - SGRPROJ_PRJ_MIN1) as u16,
        );
    }

    // C's trailing `svt_memcpy(ref_sgrproj_info, sgrproj_info, ...)`: the
    // reference for the NEXT restoration unit is this one.
    *ref_info = *info;
}
