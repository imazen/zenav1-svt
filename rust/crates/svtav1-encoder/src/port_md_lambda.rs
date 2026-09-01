//! The per-block lambda tuning and the intra tx-type helpers of
//! `Source/Lib/Codec/mode_decision.c`.
//!
//! # Coverage — 5 of the 37 rows still open on `mode_decision.c`
//!
//! | C function | line | here |
//! |---|---|---|
//! | `intra_mode_to_tx_type` | 2940 | [`intra_mode_to_tx_type`] |
//! | `svt_aom_get_intra_uv_tx_type` | 2950 | [`get_intra_uv_tx_type`] |
//! | `svt_aom_filter_intra_allowed_bsize` | 102 | [`filter_intra_allowed_bsize`] |
//! | `get_superblock_tpl_column_end` | 4046 | [`superblock_tpl_column_end`] |
//! | `svt_aom_set_tuned_blk_lambda` + `aom_av1_set_ssim_rdmult` | 4105 / 4060 | [`set_tuned_blk_lambda`] / [`set_ssim_rdmult`] |
//!
//! `svt_av1_init_me_luts` / `init_me_luts_bd` (:2044-2062) are NOT here and
//! are not a gap: their whole output is the two `sad_per_bit` tables, which
//! `crate::port_md::pme` already carries as `SAD_PER_BIT_LUT_8` /
//! `SAD_PER_BIT_LUT_10` with `get_sad_per_bit` on top.
//!
//! # The two lambda tuners return their result instead of writing four fields
//!
//! C writes `ctx->full_lambda_md[2]` and `ctx->fast_lambda_md[2]` in place,
//! and `aom_av1_set_ssim_rdmult` reads them back when
//! `blk_lambda_tuning` is set — it scales either the PICTURE lambdas or the
//! CONTEXT's current ones depending on that flag, which is exactly the kind
//! of read-modify-write that hides in a void function. Here both take the
//! current [`Lambdas`] and return the new one, so the two sources are
//! visible at the call site.
//!
//! # Cross-ISA hazard, stated up front
//!
//! Both tuners go through `libm`: `pow` in [`set_ssim_rdmult`], `log` and
//! `exp` in [`set_tuned_blk_lambda`]. Per `docs/WORKING-ON-THIS.md` §5c a
//! transcendental is the one place where a bit-exact port can still differ
//! BETWEEN hosts, and `tools/fp_cross_isa.sh` is the tool that answers it.
//! This port uses Rust's `f64::{powf, ln, exp}`, which lower to the platform
//! libm exactly as C's do — so the port inherits C's cross-ISA behaviour
//! rather than adding to it, but neither is guaranteed identical across
//! hosts. **Not measured here.** Any byte-identity claim on a picture whose
//! `blk_lambda_tuning` or tune-SSIM path is live has to run that tool first.
//!
//! # Evidence
//!
//! Tier 1 for [`filter_intra_allowed_bsize`] and [`get_intra_uv_tx_type`] —
//! both are EXPORTED (`nm -g` prints `T`) and `tests/c_parity_md_lambda.rs`
//! sweeps their whole input domains exhaustively (22 block sizes; 14 UV modes
//! x 19 tx sizes x both `reduced_tx_set` values).
//!
//! [`intra_mode_to_tx_type`] is `static` but is the only body inside
//! `get_intra_uv_tx_type`, so that sweep drives it too — for the `PLANE_UV`
//! arm. Its `PLANE_Y` arm is a direct table read and is **tier 4**.
//!
//! [`superblock_tpl_column_end`], [`set_ssim_rdmult`] and
//! [`set_tuned_blk_lambda`] are **tier 4**: the first is `static`, and the
//! two tuners need `ppcs->pa_me_data`'s two scaling-factor arrays,
//! `ed_ctx->pic_{full,fast}_lambda` and the superres/TPL geometry assembled
//! in a shim before the call reaches the arithmetic under test.

use svtav1_types::block::BlockSize;
use svtav1_types::tables::block::{
    BLOCK_SIZE_HIGH, BLOCK_SIZE_WIDE, NUM_4X4_BLOCKS_HIGH, NUM_4X4_BLOCKS_WIDE,
};

/// C `SCALE_NUMERATOR` (definitions.h:1451).
pub const SCALE_NUMERATOR: i32 = 8;
/// C `SUPERRES_INVALID_STATE` (mode_decision.c:69).
pub const SUPERRES_INVALID_STATE: u32 = 0x7fff_ffff;
/// C `TX_32X32` as a `txsize_sqr_up_map` value.
const TX_32X32: usize = 3;
/// C `DCT_DCT`.
pub const DCT_DCT: usize = 0;

/// C `g_intra_mode_to_tx_type[INTRA_MODES]` (mode_decision.c:2924) as
/// `TxType` indices: DCT_DCT 0, ADST_DCT 1, DCT_ADST 2, ADST_ADST 3.
///
/// The pattern is the mode's dominant direction: a vertical-ish mode gets
/// ADST on the column (`ADST_DCT`), a horizontal-ish one gets it on the row
/// (`DCT_ADST`), a diagonal or smooth mode gets both. D45 is the exception —
/// it reads DCT_DCT, not ADST_ADST.
pub const INTRA_MODE_TO_TX_TYPE: [usize; 13] = [
    0, // DC_PRED     -> DCT_DCT
    1, // V_PRED      -> ADST_DCT
    2, // H_PRED      -> DCT_ADST
    0, // D45_PRED    -> DCT_DCT
    3, // D135_PRED   -> ADST_ADST
    1, // D113_PRED   -> ADST_DCT
    2, // D157_PRED   -> DCT_ADST
    2, // D203_PRED   -> DCT_ADST
    1, // D67_PRED    -> ADST_DCT
    3, // SMOOTH_PRED -> ADST_ADST
    1, // SMOOTH_V    -> ADST_DCT
    2, // SMOOTH_H    -> DCT_ADST
    3, // PAETH_PRED  -> ADST_ADST
];

/// C `intra_mode_to_tx_type` (mode_decision.c:2940).
///
/// `plane_type` selects WHICH mode indexes the table: the luma mode for
/// `PLANE_TYPE_Y`, and `get_uv_mode(pred_mode_uv)` for chroma — which maps
/// `UV_CFL_PRED` onto `DC_PRED`, so a CFL block reads the DCT_DCT row.
#[inline]
pub fn intra_mode_to_tx_type(pred_mode: u8, pred_mode_uv: u8, plane_type_uv: bool) -> usize {
    let mode = if plane_type_uv {
        crate::port_rd_cost::intra_cost::UV_TO_Y_MODE[pred_mode_uv as usize]
    } else {
        pred_mode
    };
    INTRA_MODE_TO_TX_TYPE[mode as usize]
}

/// C `svt_aom_get_intra_uv_tx_type` (mode_decision.c:2950, EXPORTED).
///
/// Three gates in order: a tx bigger than 32x32 (in the SQUARE-UP mapping,
/// so 16x64 counts) is always DCT_DCT; otherwise the chroma mode picks a
/// type; and finally the type must be a member of the tx SET the size and
/// `reduced_tx_set` select, or it falls back to DCT_DCT.
///
/// C passes `DC_PRED` as the luma mode with a comment saying the argument is
/// unused — true, because `plane_type` is `PLANE_TYPE_UV`. The port passes
/// the same value so the call reads the same.
#[inline]
pub fn get_intra_uv_tx_type(pred_mode_uv: u8, tx_size: usize, reduced_tx_set: bool) -> usize {
    if crate::entropy::coeff_c::TXSIZE_SQR_UP_MAP[tx_size] > TX_32X32 {
        return DCT_DCT;
    }
    let tx_type = intra_mode_to_tx_type(0 /* DC_PRED, unused */, pred_mode_uv, true);
    let set_type = crate::entropy::coeff_c::ext_tx_set_type(tx_size, false, reduced_tx_set);
    if crate::leaf_funnel::ext_tx_used(set_type, tx_type) {
        tx_type
    } else {
        DCT_DCT
    }
}

/// C `svt_aom_filter_intra_allowed_bsize` (mode_decision.c:102, EXPORTED):
/// both dimensions at most 32 pixels.
#[inline]
pub fn filter_intra_allowed_bsize(bsize: BlockSize) -> bool {
    let i = bsize.as_index();
    BLOCK_SIZE_WIDE[i] <= 32 && BLOCK_SIZE_HIGH[i] <= 32
}

/// C `coded_to_superres_mi` (resize.h:71).
#[inline]
pub fn coded_to_superres_mi(mi_col: i32, denom: i32) -> i32 {
    (mi_col * denom + SCALE_NUMERATOR / 2) / SCALE_NUMERATOR
}

/// C `get_superblock_tpl_column_end` (mode_decision.c:4046).
///
/// The superblock's end column in TPL units. C's comment says why it exists:
/// with superres on, the superblock's end column can be off by one, so the
/// caller must clamp its column walk to this rather than to the picture.
#[inline]
pub fn superblock_tpl_column_end(
    sb_size_is_128: bool,
    superres_denom: i32,
    mi_col: i32,
    num_mi_w: i32,
) -> i32 {
    let mib_size_log2 = if sb_size_is_128 { 5 } else { 4 };
    let sb_mi_col_start = (mi_col >> mib_size_log2) << mib_size_log2;
    let sb_mi_col_start_sr = coded_to_superres_mi(sb_mi_col_start, superres_denom);
    let sb_mi_width = if sb_size_is_128 { 32 } else { 16 };
    let sb_mi_width_sr = coded_to_superres_mi(sb_mi_width, superres_denom);
    let sb_mi_end = sb_mi_col_start_sr + sb_mi_width_sr;
    (sb_mi_end + num_mi_w - 1) / num_mi_w
}

/// C's four MD lambdas: `[EB_8_BIT_MD, EB_10_BIT_MD]` for full and fast.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Lambdas {
    pub full: [u32; 2],
    pub fast: [u32; 2],
}

/// C's `(double)x * scale + 0.5` cast to `uint32_t` — a truncating cast of a
/// rounded product, NOT a rounding cast. Negative products cannot occur (both
/// factors are non-negative), so the two agree here.
#[inline]
fn scale_lambda(v: u32, scale: f64) -> u32 {
    (f64::from(v) * scale + 0.5) as u32
}

/// C `aom_av1_set_ssim_rdmult` (mode_decision.c:4060).
///
/// The geometric mean of the per-16x16 SSIM rdmult scaling factors covering
/// this block, applied to the lambdas. Which lambdas it scales depends on
/// `blk_lambda_tuning`: the PICTURE lambdas when it is off, and the CONTEXT's
/// CURRENT ones when it is on — i.e. it composes with
/// [`set_tuned_blk_lambda`]'s result rather than replacing it.
///
/// `factors` is `ppcs->pa_me_data->ssim_rdmult_scaling_factors`, indexed
/// `row * num_cols + col` over the 16x16 grid. Two of C's own quirks are
/// preserved: the row loop divides `mi_row` by `num_mi_w` (the WIDTH) and the
/// column loop divides `mi_col` by `num_mi_h`, which is a transposition that
/// is harmless only because the base block is square.
#[allow(clippy::too_many_arguments)]
pub fn set_ssim_rdmult(
    bsize: BlockSize,
    mi_row: i32,
    mi_col: i32,
    mi_rows: i32,
    mi_cols: i32,
    factors: &[f64],
    blk_lambda_tuning: bool,
    pic_lambdas: &Lambdas,
    ctx_lambdas: &Lambdas,
) -> Lambdas {
    // C `bsize_base = BLOCK_16X16` -> 4 mi units each way.
    let num_mi_w = 4i32;
    let num_mi_h = 4i32;
    let num_cols = (mi_cols + num_mi_w - 1) / num_mi_w;
    let num_rows = (mi_rows + num_mi_h - 1) / num_mi_h;
    let bw_mi = i32::from(NUM_4X4_BLOCKS_WIDE[bsize.as_index()]);
    let bh_mi = i32::from(NUM_4X4_BLOCKS_HIGH[bsize.as_index()]);
    let num_bcols = (bw_mi + num_mi_w - 1) / num_mi_w;
    let num_brows = (bh_mi + num_mi_h - 1) / num_mi_h;

    let mut num_of_mi = 0.0f64;
    let mut geom_mean_of_scale = 1.0f64;
    let row_start = mi_row / num_mi_w;
    let col_start = mi_col / num_mi_h;
    for row in row_start..num_rows.min(row_start + num_brows) {
        for col in col_start..num_cols.min(col_start + num_bcols) {
            let index = (row * num_cols + col) as usize;
            geom_mean_of_scale *= factors[index];
            num_of_mi += 1.0;
        }
    }
    geom_mean_of_scale = geom_mean_of_scale.powf(1.0 / num_of_mi);

    let src = if blk_lambda_tuning {
        ctx_lambdas
    } else {
        pic_lambdas
    };
    Lambdas {
        full: [
            scale_lambda(src.full[0], geom_mean_of_scale),
            scale_lambda(src.full[1], geom_mean_of_scale),
        ],
        fast: [
            scale_lambda(src.fast[0], geom_mean_of_scale),
            scale_lambda(src.fast[1], geom_mean_of_scale),
        ],
    }
}

/// The TPL / superres geometry `svt_aom_set_tuned_blk_lambda` walks.
#[derive(Debug, Clone, Copy)]
pub struct TplLambdaGeom {
    /// C `ctx->blk_geom->bsize`.
    pub bsize: BlockSize,
    /// C `ctx->blk_org_y / 4` and `blk_org_x / 4`.
    pub mi_row: i32,
    pub mi_col: i32,
    /// C `cm->mi_rows`.
    pub mi_rows: i32,
    /// C `ppcs->enhanced_unscaled_pic->width`.
    pub unscaled_width: i32,
    /// C `ppcs->superres_denom`.
    pub superres_denom: i32,
    /// C `ppcs->tpl_ctrls.synth_blk_size == 32`.
    pub synth_blk_32: bool,
    /// C `scs->seq_header.sb_size == BLOCK_128X128`.
    pub sb_size_is_128: bool,
}

/// C `svt_aom_set_tuned_blk_lambda` (mode_decision.c:4105, EXPORTED).
///
/// The geometric mean of the TPL rdmult scaling factors over the block,
/// computed in the LOG domain (`exp(sum(log(x)) / n)`) — unlike
/// [`set_ssim_rdmult`], which multiplies and takes a root. That difference is
/// C's and is not interchangeable in floating point.
///
/// Returns `None` for C's superres degenerate case: when superres shifts the
/// column window off the block entirely, `base_block_count` is zero and C
/// writes `SUPERRES_INVALID_STATE` into all four lambdas rather than dividing
/// by zero. C's comment names the aom counterpart that does divide by zero.
///
/// The tune-SSIM composition (`aom_av1_set_ssim_rdmult` at :4163) is the
/// caller's to apply — the gate is `tune == TUNE_SSIM || TUNE_IQ ||
/// TUNE_MS_SSIM` — because it needs a second factor array this function does
/// not take.
pub fn set_tuned_blk_lambda(
    g: &TplLambdaGeom,
    factors: &[f64],
    pic_lambdas: &Lambdas,
) -> Option<Lambdas> {
    let mi_col_sr = coded_to_superres_mi(g.mi_col, g.superres_denom);
    // C `((width + 15) / 16) << 2` — the picture's column bound in mi units.
    let mi_cols_sr = ((g.unscaled_width + 15) / 16) << 2;
    let bw_mi = i32::from(NUM_4X4_BLOCKS_WIDE[g.bsize.as_index()]);
    let bh_mi = i32::from(NUM_4X4_BLOCKS_HIGH[g.bsize.as_index()]);
    let block_mi_width_sr = coded_to_superres_mi(bw_mi, g.superres_denom);
    // C `bsize_base` is BLOCK_32X32 or BLOCK_16X16 -> 8 or 4 mi units.
    let num_mi_w = if g.synth_blk_32 { 8 } else { 4 };
    let num_mi_h = num_mi_w;
    let num_cols = (mi_cols_sr + num_mi_w - 1) / num_mi_w;
    let num_rows = (g.mi_rows + num_mi_h - 1) / num_mi_h;
    let num_bcols = (block_mi_width_sr + num_mi_w - 1) / num_mi_w;
    let num_brows = (bh_mi + num_mi_h - 1) / num_mi_h;
    let sb_bcol_end =
        superblock_tpl_column_end(g.sb_size_is_128, g.superres_denom, g.mi_col, num_mi_w);

    let mut base_block_count = 0i32;
    let mut geom_mean_of_scale = 0.0f64;
    let row_start = g.mi_row / num_mi_w;
    let col_start = mi_col_sr / num_mi_h;
    for row in row_start..num_rows.min(row_start + num_brows) {
        let col_end = num_cols.min(col_start + num_bcols).min(sb_bcol_end);
        for col in col_start..col_end {
            let index = (row * num_cols + col) as usize;
            geom_mean_of_scale += factors[index].ln();
            base_block_count += 1;
        }
    }
    if base_block_count == 0 {
        return None;
    }
    let geom_mean_of_scale = (geom_mean_of_scale / f64::from(base_block_count)).exp();
    Some(Lambdas {
        full: [
            scale_lambda(pic_lambdas.full[0], geom_mean_of_scale),
            scale_lambda(pic_lambdas.full[1], geom_mean_of_scale),
        ],
        fast: [
            scale_lambda(pic_lambdas.fast[0], geom_mean_of_scale),
            scale_lambda(pic_lambdas.fast[1], geom_mean_of_scale),
        ],
    })
}

/// The value C writes into all four lambdas in the degenerate superres case,
/// exposed so a caller can reproduce the sentinel rather than inventing one.
pub const INVALID_LAMBDAS: Lambdas = Lambdas {
    full: [SUPERRES_INVALID_STATE; 2],
    fast: [SUPERRES_INVALID_STATE; 2],
};

#[cfg(test)]
mod tests {
    use super::*;

    /// TIER 4 (mode_decision.c:2924-2944). The luma arm is a direct table
    /// read; D45 is the one directional mode that maps to DCT_DCT, and
    /// UV_CFL_PRED (13) folds onto DC through `get_uv_mode`.
    #[test]
    fn intra_mode_to_tx_type_luma_and_the_cfl_fold() {
        assert_eq!(intra_mode_to_tx_type(0, 0, false), 0); // DC -> DCT_DCT
        assert_eq!(intra_mode_to_tx_type(1, 0, false), 1); // V  -> ADST_DCT
        assert_eq!(intra_mode_to_tx_type(2, 0, false), 2); // H  -> DCT_ADST
        assert_eq!(intra_mode_to_tx_type(3, 0, false), 0); // D45 -> DCT_DCT
        assert_eq!(intra_mode_to_tx_type(12, 0, false), 3); // PAETH -> ADST_ADST
        // UV_CFL_PRED maps to DC_PRED, so it reads the DCT_DCT row — the
        // luma argument is ignored on this arm.
        assert_eq!(intra_mode_to_tx_type(12, 13, true), 0);
        assert_eq!(intra_mode_to_tx_type(0, 1, true), 1); // UV_V -> ADST_DCT
    }

    /// TIER 4 (resize.h:71 / mode_decision.c:4046). With no superres
    /// (denom == SCALE_NUMERATOR) the mi mapping is the identity, and the
    /// column end is the superblock's own end rounded up to TPL units.
    #[test]
    fn superblock_tpl_column_end_without_superres() {
        assert_eq!(coded_to_superres_mi(37, SCALE_NUMERATOR), 37);
        // 64px SB: mib_size_log2 4, sb_mi_width 16. A block at mi_col 20
        // lives in the SB starting at 16, so the end is 32 mi -> 8 TPL
        // columns at num_mi_w 4.
        assert_eq!(superblock_tpl_column_end(false, SCALE_NUMERATOR, 20, 4), 8);
        // 128px SB: log2 5, width 32. mi_col 20 -> SB start 0, end 32 -> 8.
        assert_eq!(superblock_tpl_column_end(true, SCALE_NUMERATOR, 20, 4), 8);
        // A wider TPL unit rounds up: 32 mi at num_mi_w 8 -> 4.
        assert_eq!(superblock_tpl_column_end(true, SCALE_NUMERATOR, 20, 8), 4);
    }

    /// TIER 4 (mode_decision.c:4060). An all-ones factor field leaves the
    /// lambdas untouched, and `blk_lambda_tuning` selects WHICH lambdas are
    /// scaled — the picture's or the context's.
    #[test]
    fn ssim_rdmult_scales_the_selected_lambda_source() {
        let pic = Lambdas {
            full: [1000, 2000],
            fast: [300, 400],
        };
        let ctx = Lambdas {
            full: [10, 20],
            fast: [3, 4],
        };
        let ones = vec![1.0f64; 64];
        let out = set_ssim_rdmult(
            BlockSize::Block16x16,
            0,
            0,
            64,
            64,
            &ones,
            false,
            &pic,
            &ctx,
        );
        assert_eq!(out, pic, "a unit geometric mean is the identity");
        let out = set_ssim_rdmult(BlockSize::Block16x16, 0, 0, 64, 64, &ones, true, &pic, &ctx);
        assert_eq!(out, ctx, "blk_lambda_tuning scales the CONTEXT lambdas");

        // A single covered cell of 4.0 scales everything by 4 (geometric
        // mean of one sample), with C's +0.5-then-truncate.
        let mut f = vec![1.0f64; 64];
        f[0] = 4.0;
        let out = set_ssim_rdmult(BlockSize::Block16x16, 0, 0, 64, 64, &f, false, &pic, &ctx);
        assert_eq!(out.full, [4000, 8000]);
        assert_eq!(out.fast, [1200, 1600]);
    }

    /// TIER 4 (mode_decision.c:4105-4150). The degenerate superres case —
    /// C writes SUPERRES_INVALID_STATE into all four lambdas rather than
    /// dividing by zero, which this port reports as `None`.
    #[test]
    fn tuned_blk_lambda_reports_the_superres_degenerate_case() {
        let pic = Lambdas {
            full: [1000, 2000],
            fast: [300, 400],
        };
        let ones = vec![1.0f64; 4096];
        let g = TplLambdaGeom {
            bsize: BlockSize::Block16x16,
            mi_row: 0,
            mi_col: 0,
            mi_rows: 64,
            unscaled_width: 256,
            superres_denom: SCALE_NUMERATOR,
            synth_blk_32: false,
            sb_size_is_128: false,
        };
        // A normal cell: unit factors leave the lambdas alone.
        assert_eq!(set_tuned_blk_lambda(&g, &ones, &pic), Some(pic));

        // mi_rows 0 empties the ROW loop, so base_block_count is 0 and C
        // takes the invalid-state branch.
        let mut degenerate = g;
        degenerate.mi_rows = 0;
        assert_eq!(set_tuned_blk_lambda(&degenerate, &ones, &pic), None);
        assert_eq!(INVALID_LAMBDAS.full[0], SUPERRES_INVALID_STATE);
    }
}
