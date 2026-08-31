//! The fast RD models that rank inter candidates.
//!
//! Ported from `Source/Lib/Codec/enc_inter_prediction.c` (SVT-AV1 v4.2.0):
//! `sse_norm_curvfit_model_cat_lookup` (:200), `av1_model_rd_curvfit` (:294),
//! `model_rd_with_curvfit` (:316), `model_rd_norm` (:1877),
//! `svt_av1_model_rd_from_var_lapndz` (:1933) and `model_rd_from_sse` (:1954).
//!
//! These decide which inter candidate survives, so they are bit-affecting even
//! though they compute no pixels.
//!
//! # The determinism worry, and why it does not apply
//!
//! The triage that queued this cluster flagged it as a cross-ISA
//! floating-point risk because `model_rd_with_curvfit` "calls `svt_log2f_safe`
//! and uses `f64`". MEASURED: `svt_log2f_safe(x)` is
//! `get_msb((x) | 1)` (definitions.h:612) — an **integer** most-significant-bit
//! index, not `log2f`. There is no transcendental anywhere in this cluster.
//!
//! The `double`s that remain are: a division by `num_samples`, a table lookup
//! by an integer index, one multiply, and two `+ 0.5` truncations. Every one
//! of those is an IEEE-754 basic operation, which is exactly specified and
//! identical on every conforming target — unlike a libm call. So
//! `tools/fp_cross_isa.sh` has nothing to find here; the risk the triage named
//! is not present. (The tables themselves are `f64` constants transcribed
//! verbatim, so they carry no rounding of their own.)

use svtav1_types::block::BlockSize;

/// `AV1_PROB_COST_SHIFT` (md_rate_estimation.h:29).
pub const AV1_PROB_COST_SHIFT: u32 = 9;
/// `RDDIV_BITS` (rd_cost.h:34).
pub const RDDIV_BITS: u32 = 7;

/// `eb_num_pels_log2_lookup` (common_utils.c:39).
pub const NUM_PELS_LOG2_LOOKUP: [u32; BlockSize::SIZES_ALL] = [
    4, 5, 5, 6, 7, 7, 8, 9, 9, 10, 11, 11, 12, 13, 13, 14, 6, 6, 8, 8, 10, 10,
];

/// `bsize_curvfit_model_cat_lookup` (enc_inter_prediction.c:196).
pub const BSIZE_CURVFIT_MODEL_CAT_LOOKUP: [usize; BlockSize::SIZES_ALL] = [
    0, 0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 3, 3, 3, 0, 0, 1, 1, 2, 2,
];

/// `get_msb(n)` (definitions.h:617) — the index of the most significant set
/// bit. C asserts `n != 0`; this panics instead of returning a wrong index.
pub fn get_msb(n: u32) -> i32 {
    assert!(n != 0, "get_msb is only defined for n != 0");
    31 - n.leading_zeros() as i32
}

/// `svt_log2f_safe(x)` (definitions.h:612) — `get_msb(x | 1)`. Integer, not
/// floating point; see the module doc.
pub fn log2f_safe(x: u32) -> i32 {
    get_msb(x | 1)
}

/// `ROUND_POWER_OF_TWO` on an i64.
#[inline]
fn round_power_of_two_i64(value: i64, n: u32) -> i64 {
    if n == 0 {
        value
    } else {
        (value + (1i64 << (n - 1))) >> n
    }
}

/// `RDCOST(RM, R, D)` (rd_cost.h:36).
pub fn rdcost(rate_mult: u32, rate: i64, dist: i64) -> i64 {
    round_power_of_two_i64(rate * rate_mult as i64, AV1_PROB_COST_SHIFT) + (dist << RDDIV_BITS)
}

/// `sse_norm_curvfit_model_cat_lookup` (enc_inter_prediction.c:200) — one
/// comparison, but it picks a different 65-entry distortion table.
pub fn sse_norm_curvfit_model_cat_lookup(sse_norm: f64) -> usize {
    usize::from(sse_norm > 16.0)
}

/// `interp_rgrid_curv` (enc_inter_prediction.c:203) — 4 categories x 65.
pub const INTERP_RGRID_CURV: [[f64; 65]; 4] = [
    [
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        23.801499,
        28.387688,
        33.388795,
        42.298282,
        41.525408,
        51.597692,
        49.566271,
        54.632979,
        60.321507,
        67.730678,
        75.766165,
        85.324032,
        96.600012,
        120.839562,
        173.917577,
        255.974908,
        354.107573,
        458.063476,
        562.345966,
        668.568424,
        772.072881,
        878.598490,
        982.202274,
        1082.708946,
        1188.037853,
        1287.702240,
        1395.588773,
        1490.825830,
        1584.231230,
        1691.386090,
        1766.822555,
        1869.630904,
        1926.743565,
        2002.949495,
        2047.431137,
        2138.486068,
        2154.743767,
        2209.242472,
        2277.593051,
        2290.996432,
        2307.452938,
        2343.567091,
        2397.654644,
        2469.425868,
        2558.591037,
        2664.860422,
        2787.944296,
        2927.552932,
        3083.396602,
        3255.185579,
        3442.630134,
        3645.440541,
        3863.327072,
        4096.000000,
    ],
    [
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        8.998436,
        9.439592,
        9.731837,
        10.865931,
        11.561347,
        12.578139,
        14.205101,
        16.770584,
        19.094853,
        21.330863,
        23.298907,
        26.901921,
        34.501017,
        57.891733,
        112.234763,
        194.853189,
        288.302032,
        380.499422,
        472.625309,
        560.226809,
        647.928463,
        734.155122,
        817.489721,
        906.265783,
        999.260562,
        1094.489206,
        1197.062998,
        1293.296825,
        1378.926484,
        1472.760990,
        1552.663779,
        1635.196884,
        1692.451951,
        1759.741063,
        1822.162720,
        1916.515921,
        1966.686071,
        2031.647506,
        2033.700134,
        2087.847688,
        2161.688858,
        2242.536028,
        2334.023491,
        2436.337802,
        2549.665519,
        2674.193198,
        2810.107395,
        2957.594666,
        3116.841567,
        3288.034655,
        3471.360486,
        3667.005616,
        3875.156602,
        4096.000000,
    ],
    [
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        2.377584,
        2.557185,
        2.732445,
        2.851114,
        3.281800,
        3.765589,
        4.342578,
        5.145582,
        5.611038,
        6.642238,
        7.945977,
        11.800522,
        17.346624,
        37.501413,
        87.216800,
        165.860942,
        253.865564,
        332.039345,
        408.518863,
        478.120452,
        547.268590,
        616.067676,
        680.022540,
        753.863541,
        834.529973,
        919.489191,
        1008.264989,
        1092.230318,
        1173.971886,
        1249.514122,
        1330.510941,
        1399.523249,
        1466.923387,
        1530.533471,
        1586.515722,
        1695.197774,
        1746.648696,
        1837.136959,
        1909.075485,
        1975.074651,
        2060.159200,
        2155.335095,
        2259.762505,
        2373.710437,
        2497.447898,
        2631.243895,
        2775.367434,
        2930.087523,
        3095.673170,
        3272.393380,
        3460.517161,
        3660.313520,
        3872.051464,
        4096.000000,
    ],
    [
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.000000,
        0.296997,
        0.342545,
        0.403097,
        0.472889,
        0.614483,
        0.842937,
        1.050824,
        1.326663,
        1.717750,
        2.530591,
        3.582302,
        6.995373,
        9.973335,
        24.042464,
        56.598240,
        113.680735,
        180.018689,
        231.050567,
        266.101082,
        294.957934,
        323.326511,
        349.434429,
        380.443211,
        408.171987,
        441.214916,
        475.716772,
        512.900000,
        551.186939,
        592.364455,
        624.527378,
        661.940693,
        679.185473,
        724.800679,
        764.781792,
        873.050019,
        950.299001,
        939.292954,
        1052.406153,
        1033.893184,
        1112.182406,
        1219.174326,
        1337.296681,
        1471.648357,
        1622.492809,
        1790.093491,
        1974.713858,
        2176.617364,
        2396.067465,
        2633.327614,
        2888.661266,
        3162.331876,
        3454.602899,
        3765.737789,
        4096.000000,
    ],
];

/// `interp_dgrid_curv` (enc_inter_prediction.c:265) — 2 categories x 65.
pub const INTERP_DGRID_CURV: [[f64; 65]; 2] = [
    [
        16.000000, 15.962891, 15.925174, 15.886888, 15.848074, 15.808770, 15.769015, 15.728850,
        15.688313, 15.647445, 15.606284, 15.564870, 15.525918, 15.483820, 15.373330, 15.126844,
        14.637442, 14.184387, 13.560070, 12.880717, 12.165995, 11.378144, 10.438769, 9.130790,
        7.487633, 5.688649, 4.267515, 3.196300, 2.434201, 1.834064, 1.369920, 1.035921, 0.775279,
        0.574895, 0.427232, 0.314123, 0.233236, 0.171440, 0.128188, 0.092762, 0.067569, 0.049324,
        0.036330, 0.027008, 0.019853, 0.015539, 0.011093, 0.008733, 0.007624, 0.008105, 0.005427,
        0.004065, 0.003427, 0.002848, 0.002328, 0.001865, 0.001457, 0.001103, 0.000801, 0.000550,
        0.000348, 0.000193, 0.000085, 0.000021, 0.000000,
    ],
    [
        16.000000, 15.996116, 15.984769, 15.966413, 15.941505, 15.910501, 15.873856, 15.832026,
        15.785466, 15.734633, 15.679981, 15.621967, 15.560961, 15.460157, 15.288367, 15.052462,
        14.466922, 13.921212, 13.073692, 12.222005, 11.237799, 9.985848, 8.898823, 7.423519,
        5.995325, 4.773152, 3.744032, 2.938217, 2.294526, 1.762412, 1.327145, 1.020728, 0.765535,
        0.570548, 0.425833, 0.313825, 0.232959, 0.171324, 0.128174, 0.092750, 0.067558, 0.049319,
        0.036330, 0.027008, 0.019853, 0.015539, 0.011093, 0.008733, 0.007624, 0.008105, 0.005427,
        0.004065, 0.003427, 0.002848, 0.002328, 0.001865, 0.001457, 0.001103, 0.000801, 0.000550,
        0.000348, 0.000193, 0.000085, 0.000021, -0.000000,
    ],
];

/// `av1_model_rd_curvfit` (enc_inter_prediction.c:294).
///
/// The long comment above it describes a cubic interpolation; the code does
/// NOT do one — it reads `prate[1]`, i.e. `interp_rgrid_curv[rcat][xi]`, a
/// plain table lookup after a clamp and a floor. Porting the comment instead
/// of the code would change every candidate ranking.
pub fn model_rd_curvfit(bsize: BlockSize, sse_norm: f64, xqr: f64) -> (f64, f64) {
    let x_start = -15.5f64;
    let x_end = 16.5f64;
    let x_step = 0.5f64;
    let epsilon = 1e-6f64;
    let rcat = BSIZE_CURVFIT_MODEL_CAT_LOOKUP[bsize as usize];
    let dcat = sse_norm_curvfit_model_cat_lookup(sse_norm);

    let mut xqr = xqr.max(x_start + x_step + epsilon);
    xqr = xqr.min(x_end - x_step - epsilon);
    let x = (xqr - x_start) / x_step;
    let xi = x.floor() as usize;
    debug_assert!(xi > 0);

    (INTERP_RGRID_CURV[rcat][xi], INTERP_DGRID_CURV[dcat][xi])
}

/// `model_rd_with_curvfit` (enc_inter_prediction.c:316).
///
/// C reaches into `PictureControlSet` / `ModeDecisionContext` only to fetch
/// `dequants->y_dequant_qtx[base_q_idx][1]`; that value arrives here as
/// `quantizer` so the function stays a pure computation.
///
/// TRAP: `xqr` is `svt_log2f_safe((uint32_t)sse_norm / (qstep * qstep))` — the
/// `double` `sse_norm` is TRUNCATED to `uint32_t` first and the division is
/// INTEGER. Computing `log2(sse_norm / (qstep*qstep))` in floating point
/// instead gives a different index for most inputs.
pub fn model_rd_with_curvfit(
    plane_bsize: BlockSize,
    sse: i64,
    num_samples: i32,
    quantizer: i16,
    rdmult: u32,
) -> (i32, i64) {
    let dequant_shift = 3;
    let qstep = (quantizer >> dequant_shift).max(1) as i64;

    if sse == 0 {
        return (0, 0);
    }

    let sse_norm = sse as f64 / num_samples as f64;
    let xqr = log2f_safe((sse_norm as u32) / (qstep * qstep) as u32) as f64;

    let (rate_f, dist_by_sse_norm_f) = model_rd_curvfit(plane_bsize, sse_norm, xqr);

    let dist_f = dist_by_sse_norm_f * sse_norm;
    let mut rate_i = ((rate_f * num_samples as f64) + 0.5) as i32;
    let mut dist_i = ((dist_f * num_samples as f64) + 0.5) as i64;

    // "Check if skip is better" — note the FIRST branch overwrites dist even
    // though rate is already 0, so a zero-rate result always carries sse << 4.
    if rate_i == 0 {
        dist_i = sse << 4;
    } else if rdcost(rdmult, rate_i as i64, dist_i) >= rdcost(rdmult, 0, sse << 4) {
        rate_i = 0;
        dist_i = sse << 4;
    }

    (rate_i, dist_i)
}

/// `rate_tab_q10` (enc_inter_prediction.c:1890).
const RATE_TAB_Q10: [i32; 104] = [
    65536, 6086, 5574, 5275, 5063, 4899, 4764, 4651, 4553, 4389, 4255, 4142, 4044, 3958, 3881,
    3811, 3748, 3635, 3538, 3453, 3376, 3307, 3244, 3186, 3133, 3037, 2952, 2877, 2809, 2747, 2690,
    2638, 2589, 2501, 2423, 2353, 2290, 2232, 2179, 2130, 2084, 2001, 1928, 1862, 1802, 1748, 1698,
    1651, 1608, 1530, 1460, 1398, 1342, 1290, 1243, 1199, 1159, 1086, 1021, 963, 911, 864, 821,
    781, 745, 680, 623, 574, 530, 490, 455, 424, 395, 345, 304, 269, 239, 213, 190, 171, 154, 126,
    104, 87, 73, 61, 52, 44, 38, 28, 21, 16, 12, 10, 8, 6, 5, 3, 2, 1, 1, 1, 0, 0,
];

/// `dist_tab_q10` (enc_inter_prediction.c:1905).
const DIST_TAB_Q10: [i32; 104] = [
    0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 4, 5, 5, 6, 7, 7, 8, 9, 11, 12, 13, 15, 16, 17, 18, 21, 24, 26,
    29, 31, 34, 36, 39, 44, 49, 54, 59, 64, 69, 73, 78, 88, 97, 106, 115, 124, 133, 142, 151, 167,
    184, 200, 215, 231, 245, 260, 274, 301, 327, 351, 375, 397, 418, 439, 458, 495, 528, 559, 587,
    613, 637, 659, 680, 717, 749, 777, 801, 823, 842, 859, 874, 899, 919, 936, 949, 960, 969, 977,
    983, 994, 1001, 1006, 1010, 1013, 1015, 1017, 1018, 1020, 1022, 1022, 1023, 1023, 1023, 1024,
];

/// `xsq_iq_q10` (enc_inter_prediction.c:1914).
const XSQ_IQ_Q10: [i32; 104] = [
    0, 4, 8, 12, 16, 20, 24, 28, 32, 40, 48, 56, 64, 72, 80, 88, 96, 112, 128, 144, 160, 176, 192,
    208, 224, 256, 288, 320, 352, 384, 416, 448, 480, 544, 608, 672, 736, 800, 864, 928, 992, 1120,
    1248, 1376, 1504, 1632, 1760, 1888, 2016, 2272, 2528, 2784, 3040, 3296, 3552, 3808, 4064, 4576,
    5088, 5600, 6112, 6624, 7136, 7648, 8160, 9184, 10208, 11232, 12256, 13280, 14304, 15328,
    16352, 18400, 20448, 22496, 24544, 26592, 28640, 30688, 32736, 36832, 40928, 45024, 49120,
    53216, 57312, 61408, 65504, 73696, 81888, 90080, 98272, 106464, 114656, 122848, 131040, 147424,
    163808, 180192, 196576, 212960, 229344, 245728,
];

/// `model_rd_norm` (enc_inter_prediction.c:1877) — the q10 fixed-point table
/// interpolation inside `svt_av1_model_rd_from_var_lapndz`.
pub fn model_rd_norm(xsq_q10: i32) -> (i32, i32) {
    let tmp = (xsq_q10 >> 2) + 8;
    let k = get_msb(tmp as u32) - 3;
    let xq = ((k << 3) + ((tmp >> k) & 0x7)) as usize;
    let one_q10 = 1 << 10;
    let a_q10 = ((xsq_q10 - XSQ_IQ_Q10[xq]) << 10) >> (2 + k);
    let b_q10 = one_q10 - a_q10;
    let r_q10 = (RATE_TAB_Q10[xq] * b_q10 + RATE_TAB_Q10[xq + 1] * a_q10) >> 10;
    let d_q10 = (DIST_TAB_Q10[xq] * b_q10 + DIST_TAB_Q10[xq + 1] * a_q10) >> 10;
    (r_q10, d_q10)
}

/// `MAX_XSQ_Q10` (enc_inter_prediction.c:1946).
pub const MAX_XSQ_Q10: u64 = 245727;

/// `svt_av1_model_rd_from_var_lapndz` (enc_inter_prediction.c:1933).
pub fn model_rd_from_var_lapndz(var: i64, n_log2: u32, qstep: u32) -> (i32, i64) {
    if var == 0 {
        return (0, 0);
    }
    let xsq_q10_64 =
        ((((qstep as u64) * (qstep as u64)) << (n_log2 + 10)) + (var >> 1) as u64) / var as u64;
    let xsq_q10 = xsq_q10_64.min(MAX_XSQ_Q10) as i32;
    let (r_q10, d_q10) = model_rd_norm(xsq_q10);
    let rate = round_power_of_two_i64((r_q10 as i64) << n_log2, 10 - AV1_PROB_COST_SHIFT) as i32;
    let dist = (var * d_q10 as i64 + 512) >> 10;
    (rate, dist)
}

/// `model_rd_from_sse` (enc_inter_prediction.c:1954).
///
/// TRAP: the `simple_model_rd_from_var` arm reassigns `quantizer` in place
/// (`quantizer = quantizer >> dequant_shift`) and then compares the SHIFTED
/// value against 120 and multiplies by it. The `else` arm shifts too, but by
/// passing `quantizer >> dequant_shift`. Both shift; only the fast arm mutates.
///
/// `*dist <<= 4` happens on BOTH arms, after the branch.
pub fn model_rd_from_sse(
    bsize: BlockSize,
    quantizer: i16,
    bit_depth: u8,
    sse: u64,
    simple_model_rd_from_var: bool,
) -> (u32, u64) {
    let dequant_shift = bit_depth as i32 - 5;
    let (rate, mut dist): (u32, u64);
    if simple_model_rd_from_var {
        let square_error = sse as i64;
        let q = (quantizer >> dequant_shift) as i64;
        rate = if q < 120 {
            ((square_error * (280 - q)) >> (16 - AV1_PROB_COST_SHIFT)) as i32 as u32
        } else {
            0
        };
        dist = ((square_error * q) >> 8) as u64;
    } else {
        let (r, d) = model_rd_from_var_lapndz(
            sse as i64,
            NUM_PELS_LOG2_LOOKUP[bsize as usize],
            (quantizer >> dequant_shift) as u32,
        );
        rate = r as u32;
        dist = d as u64;
    }
    dist <<= 4;
    (rate, dist)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `svt_log2f_safe` is an integer MSB, not a float log2 — the fact the
    /// module doc rests on.
    #[test]
    fn log2f_safe_is_integer_msb() {
        assert_eq!(log2f_safe(0), 0); // (0 | 1) -> msb 0
        assert_eq!(log2f_safe(1), 0);
        assert_eq!(log2f_safe(2), 1);
        assert_eq!(log2f_safe(3), 1);
        assert_eq!(log2f_safe(255), 7);
        assert_eq!(log2f_safe(256), 8);
        assert_eq!(log2f_safe(u32::MAX), 31);
    }

    /// A zero variance short-circuits to zero rate AND zero distortion.
    #[test]
    fn zero_var_is_zero_cost() {
        assert_eq!(model_rd_from_var_lapndz(0, 8, 10), (0, 0));
        assert_eq!(
            model_rd_with_curvfit(BlockSize::Block8x8, 0, 64, 100, 500),
            (0, 0)
        );
    }

    /// The three q10 tables must be the same length — C says so in a comment
    /// and then indexes `xq + 1` without a bound.
    #[test]
    fn q10_tables_are_the_same_length() {
        assert_eq!(RATE_TAB_Q10.len(), DIST_TAB_Q10.len());
        assert_eq!(RATE_TAB_Q10.len(), XSQ_IQ_Q10.len());
        // The largest xq reachable from MAX_XSQ_Q10 must leave room for xq+1.
        let tmp = ((MAX_XSQ_Q10 as i32) >> 2) + 8;
        let k = get_msb(tmp as u32) - 3;
        let xq = ((k << 3) + ((tmp >> k) & 0x7)) as usize;
        assert!(
            xq + 1 < RATE_TAB_Q10.len(),
            "xq {xq} would read past the table"
        );
    }
}
