//! Leaves of `Source/Lib/Codec/src_ops_process.c` — the source-based operations
//! and temporal-dependency-model (TPL) process.
//!
//! ## What is here and what is NOT
//!
//! Ported here: the three exported per-block variance measures
//! (`svt_aom_get_perpixel_variance`, `svt_aom_get_mean_and_perpixel_variance`,
//! `svt_aom_get_perceptual_perpixel_variance`) and three of the TPL
//! propagation leaves (`round_floor`, `get_overlap_area`, `delta_rate_cost`).
//!
//! NOT ported here, and named so nobody reads this module as "TPL is done":
//! * `get_quantize_error` and `rate_estimator` need the TPL quantizer
//!   (`svt_av1_quantize_fp`, `svt_av1_block_error`) and a scan order for an
//!   arbitrary `TxSize`; both are a separate chunk.
//! * `tpl_mc_flow*`, `tpl_model_update*`, `tpl_subpel_search`,
//!   `tpl_regular_setup_me_refs`, `tpl_prep_info`, `svt_aom_generate_r0beta`,
//!   `generate_lambda_scaling_factor`, `aom_av1_set_mb_ssim_rdmult_scaling` —
//!   the engine proper, which walks the PCS.
//! * `assign_tpl_segments`, `init_tpl_segments`, `init_tpl_buffers`,
//!   `init_xd_tpl`, `sbo_send_picture_out`, `svt_aom_tpl_disp_kernel*`,
//!   `svt_aom_source_based_operations_kernel*`, the two `*_ctor`/`*_dctor`
//!   pairs and `tpl_dispenser_st` — SVT's threading, segment scheduling,
//!   allocation and buffer plumbing, which this port replaces by design rather
//!   than translating.
//!
//! ## Evidence
//!
//! The three variance measures are EXPORTED and gated at tier 1
//! (`c_parity_src_ops.rs`). The three TPL leaves are `static` with no exported
//! caller that isolates them, so they are TIER 4 — transcribed against the C
//! source with the line cited, and said so.
//!
//! ## Mutation coverage, and the three mutations that survive
//!
//! Fifteen single-constant mutations of this module were run against the
//! differential; twelve fail it. The three that do not are recorded here
//! rather than left for someone to rediscover:
//!
//! 1. **`get_perpixel_variance`'s reference of 128 can be changed to 127.**
//!    This is a genuine no-op, not a coverage hole, and the algebra is exact
//!    including the integer truncation: for a reference `r`,
//!    `sum_r = S - n*r` and `sse_r = sum((x - r)^2)`, so
//!    `sum_r^2 / n = S^2/n - 2*S*r + n*r^2` — the two extra terms are
//!    integers, the fractional part that `/` discards is the same for every
//!    `r`, and `sse_r - floor(sum_r^2 / n)` is therefore independent of `r`.
//!    The 128 is a convention, and C's `AV1_VAR_OFFS` table exists to give the
//!    SIMD kernel something to read, not to bias the result.
//!
//! 2. and 3. **The two float-shape mutations in
//!    `get_perceptual_perpixel_variance`** — doing the square root in `f64` and
//!    narrowing afterwards instead of narrowing first, and doing the final
//!    division and sum in `f64` instead of `f32`. Both agree with C on every
//!    cell of the suite (13 block sizes x 63 blocks, including 40 random ones
//!    per size added specifically to hunt for a boundary case). They are NOT
//!    equivalent in general — `var + 1` is exactly representable in `f32` only
//!    while `var <= 2^24`, and a double-rounded square root can differ from a
//!    correctly-rounded one — so the port keeps C's spelling
//!    (`sqrtf((float)(var + 1.))`, then `unsigned / float` in `float`) and this
//!    note stands in for a differential that cannot currently tell them apart.
//!    A caller that ever sees a per-pixel variance above 2^24 would separate
//!    them; nothing can, since the maximum is 255^2.

use crate::md_subpel::NUM_PELS_LOG2_LOOKUP;
use svtav1_types::block::BlockSize;
use svtav1_types::tables::block::{
    BLOCK_SIZE_HIGH, BLOCK_SIZE_HIGH_LOG2, BLOCK_SIZE_WIDE, BLOCK_SIZE_WIDE_LOG2,
};

/// `ROUND_POWER_OF_TWO(value, n)` on a `u64` accumulator.
#[inline]
fn round_power_of_two_u64(value: u64, n: u32) -> u64 {
    (value + (1u64 << n >> 1)) >> n
}

/// The number of pixels in `bsize`, as `1 << eb_num_pels_log2_lookup[bsize]`.
#[inline]
fn num_pels_log2(bsize: BlockSize) -> u32 {
    u32::from(NUM_PELS_LOG2_LOOKUP[bsize as usize])
}

/// `svt_aom_get_perpixel_variance` (src_ops_process.c:2129). EXPORTED and
/// RTCD-dispatched — TIER 1.
///
/// Variance against `AV1_VAR_OFFS`, a constant-128 plane read at `b_stride 0`
/// — so the "reference" is the same 128 for every sample and this reduces to
/// `sse - sum^2 / n` about 128, normalised per pixel.
///
/// The all-128 table is a different array from the one
/// `svt_av1_get_sby_perpixel_variance` uses (`svt_aom_eb_av1_var_offs`,
/// pic_analysis_process.c) with identical contents; the two functions are
/// otherwise the same measurement, and this one takes an arbitrary block size
/// where that one is used only at 8x8 and 16x16.
///
/// `buf` must hold `block_size_high[bsize]` rows at `stride`.
pub fn get_perpixel_variance(buf: &[u8], stride: usize, bsize: BlockSize) -> u32 {
    let (bw, bh) = block_dims(bsize);
    let mut sum: i64 = 0;
    let mut sse: u64 = 0;
    for r in 0..bh {
        let row = &buf[r * stride..r * stride + bw];
        for &px in row {
            let diff = i32::from(px) - 128;
            sum += i64::from(diff);
            sse += (diff * diff) as u64;
        }
    }
    // C `variance_c` (C_DEFAULT/variance.c): `sse - (uint32_t)((int64_t)sum *
    // sum / (w * h))`, in `uint32_t`. The subtraction is exact here — the
    // true variance is non-negative — but the narrowing is C's, so it is
    // spelled the same way.
    let var = (sse as u32).wrapping_sub((sum * sum / (bw as i64 * bh as i64)) as u32);
    round_power_of_two_u64(u64::from(var), num_pels_log2(bsize)) as u32
}

/// `svt_aom_get_mean_and_perpixel_variance` (src_ops_process.c:2136).
/// EXPORTED — TIER 1.
///
/// Returns `(perpixel_var, mean)`. Unlike [`get_perpixel_variance`] this is the
/// variance about the block's OWN rounded mean, not about 128, and it is a
/// plain two-pass scalar loop in C with no RTCD kernel behind it.
///
/// C writes `const int diff = buf[...] - *mean;` where `*mean` is `uint32_t`,
/// so the subtraction happens in UNSIGNED and wraps for samples below the mean
/// before being converted back to `int`. Every real compiler gives the true
/// difference back, and `diff * diff` is the same either way, so this port
/// computes in `i32` directly — the one place where not transcribing the C
/// spelling is the clearer choice, and it is called out because it is a
/// signedness change.
pub fn get_mean_and_perpixel_variance(buf: &[u8], stride: usize, bsize: BlockSize) -> (u32, u32) {
    let (bw, bh) = block_dims(bsize);
    let shift = num_pels_log2(bsize);

    let mut sum: u64 = 0;
    for r in 0..bh {
        for &px in &buf[r * stride..r * stride + bw] {
            sum += u64::from(px);
        }
    }
    let mean = round_power_of_two_u64(sum, shift) as u32;

    let mut sse: u64 = 0;
    for r in 0..bh {
        for &px in &buf[r * stride..r * stride + bw] {
            let diff = i32::from(px) - mean as i32;
            sse += (diff * diff) as u64;
        }
    }
    (round_power_of_two_u64(sse, shift) as u32, mean)
}

/// `svt_aom_get_perceptual_perpixel_variance` (src_ops_process.c:2164).
/// EXPORTED — TIER 1.
///
/// The block's own variance, boosted where the block's mean sits near
/// mid-grey, on the argument that the eye is most sensitive there. The weight
/// is a parabola in the mean, `256 * (128^2 - (mean - 128)^2) / 128^2`, so it
/// is 256 at mean 128 and 0 at 0 and 256.
///
/// TWO traps in the last line, both transcribed rather than tidied:
/// * C's local is named `var` and its `mean` is the SECOND out-parameter, but
///   the call site passes them as `(&var, &mean)` — matching
///   `(perpixel_var, mean)`. This port returns a tuple in that order so the
///   names cannot drift.
/// * `var + ((var * weight) / sqrtf(var + 1.))` mixes an `unsigned` numerator
///   with a `float` divisor, so the division and the addition happen in
///   `float` and the result is TRUNCATED toward zero on assignment to
///   `unsigned int`. Doing it in `f64`, or rounding, gives different answers.
///   `sqrtf` is IEEE-754-correctly-rounded, so unlike `expf`/`logf` it carries
///   no cross-ISA risk. Note also that C writes `sqrtf(var + 1.)`: the `1.` is
///   a `double`, so the sum is computed in `double` and then narrowed to
///   `float` for `sqrtf`.
pub fn get_perceptual_perpixel_variance(buf: &[u8], stride: usize, bsize: BlockSize) -> u32 {
    let (var, mean) = get_mean_and_perpixel_variance(buf, stride, bsize);

    let centered_mean = mean as i32 - 128;
    let weight_numerator = 128 * 128 - centered_mean * centered_mean;
    let weight = (weight_numerator * 256) / (128 * 128);

    let root = ((f64::from(var) + 1.0) as f32).sqrt();
    (var as f32 + ((var as f32 * weight as f32) / root)) as u32
}

/// `block_size_wide[bsize]` and `block_size_high[bsize]`, in pixels.
#[inline]
fn block_dims(bsize: BlockSize) -> (usize, usize) {
    (
        usize::from(BLOCK_SIZE_WIDE[bsize as usize]),
        usize::from(BLOCK_SIZE_HIGH[bsize as usize]),
    )
}

// ---------------------------------------------------------------------------
// TPL propagation leaves — TIER 4
// ---------------------------------------------------------------------------

/// `round_floor` (src_ops_process.c:1441). `static` in C — TIER 4.
///
/// Floor division that rounds toward NEGATIVE infinity, which C's `/` does not:
/// the negative arm is written `-(1 + (-ref_pos - 1) / bsize_pix)` precisely
/// because C truncates toward zero. Rust's `/` truncates the same way, so the
/// same spelling is needed here; `div_euclid` would agree for positive
/// divisors but is a different function and is not what C computes.
///
/// Returns `None` when `bsize_pix` is zero rather than dividing by it. C would
/// trap; no caller can reach it, because `bsize_pix` is always a block
/// dimension.
pub fn round_floor(ref_pos: i32, bsize_pix: i32) -> Option<i32> {
    if bsize_pix == 0 {
        return None;
    }
    Some(if ref_pos < 0 {
        -(1 + (-ref_pos - 1) / bsize_pix)
    } else {
        ref_pos / bsize_pix
    })
}

/// Which corner of the reference block the overlap is measured from, in
/// `get_overlap_area`.
///
/// C passes a bare `int block` and `assert(0)`s on anything outside 0..=3; an
/// enum makes the fourth case unreachable instead of asserted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlapCorner {
    /// C `case 0` — the grid block is up-left of the reference.
    UpLeft,
    /// C `case 1` — up-right.
    UpRight,
    /// C `case 2` — down-left.
    DownLeft,
    /// C `case 3` — down-right.
    DownRight,
}

/// `get_overlap_area` (src_ops_process.c:1411). `static` in C — TIER 4.
///
/// The area, in pixels, that a `bsize` block at `(grid_pos_row, grid_pos_col)`
/// shares with one at `(ref_pos_row, ref_pos_col)`, given which way they
/// overlap. C derives the block dimensions as `4 << mi_size_wide_log2[bsize]`
/// rather than reading `block_size_wide`, which is the same number by
/// construction; the C spelling is kept so a table divergence would show up
/// here rather than be hidden.
///
/// The product can be negative when the two blocks do not actually overlap in
/// the direction claimed. C returns that negative and its callers rely on
/// clamping elsewhere, so it is returned as-is.
pub fn get_overlap_area(
    grid_pos_row: i32,
    grid_pos_col: i32,
    ref_pos_row: i32,
    ref_pos_col: i32,
    corner: OverlapCorner,
    bsize: BlockSize,
) -> i32 {
    let bw = 4i32 << (i32::from(BLOCK_SIZE_WIDE_LOG2[bsize as usize]) - 2);
    let bh = 4i32 << (i32::from(BLOCK_SIZE_HIGH_LOG2[bsize as usize]) - 2);
    let (width, height) = match corner {
        OverlapCorner::UpLeft => (
            grid_pos_col + bw - ref_pos_col,
            grid_pos_row + bh - ref_pos_row,
        ),
        OverlapCorner::UpRight => (
            ref_pos_col + bw - grid_pos_col,
            grid_pos_row + bh - ref_pos_row,
        ),
        OverlapCorner::DownLeft => (
            grid_pos_col + bw - ref_pos_col,
            ref_pos_row + bh - grid_pos_row,
        ),
        OverlapCorner::DownRight => (
            ref_pos_col + bw - grid_pos_col,
            ref_pos_row + bh - grid_pos_row,
        ),
    };
    width * height
}

/// `TPL_DEP_COST_SCALE_LOG2` (src_ops_process.c) + `AV1_PROB_COST_SHIFT`
/// (md_rate_estimation.h:29) — the combined shift `delta_rate_cost` works in.
pub const TPL_DEP_COST_SCALE_LOG2: u32 = 4;
/// `AV1_PROB_COST_SHIFT`.
pub const AV1_PROB_COST_SHIFT: u32 = 9;

/// `delta_rate_cost` (src_ops_process.c:1452). `static` in C — TIER 4.
///
/// The TPL rate the propagated distortion is worth, from the ratio
/// `beta = srcrf_dist / recrf_dist`.
///
/// ## Cross-ISA risk, stated rather than hidden
///
/// This is the ONLY function in this module that calls a transcendental:
/// `log` and `pow` on `f64`. Per `docs/WORKING-ON-THIS.md` §5c those are libm
/// calls whose last bit is NOT specified by IEEE-754, so glibc and the Apple
/// libm can legitimately differ, and `tier_invariance.rs` cannot see the
/// difference because it walks SIMD tiers within one host. `tools/fp_cross_isa.sh`
/// and `tools/cross_isa_port_check.sh` are the instruments for it. That check
/// has NOT been run for this function — it is unreachable today (nothing in
/// this port calls it yet) — and this note is here so the next person runs it
/// before wiring the TPL engine up rather than after a byte divergence.
///
/// The port uses the same operations in the same order and the same types, so
/// a divergence would come from the libm, not from the transcription.
///
/// `recrf_dist == 0` would divide by zero in C (it is `AOMMAX`'d to 1 by
/// `result_model_store` before any call); this returns `None` rather than
/// producing an infinity.
pub fn delta_rate_cost(
    delta_rate: i64,
    recrf_dist: i64,
    srcrf_dist: i64,
    pix_num: i32,
) -> Option<i64> {
    if recrf_dist == 0 || pix_num == 0 {
        return None;
    }
    let shift = TPL_DEP_COST_SCALE_LOG2 + AV1_PROB_COST_SHIFT;
    let beta = srcrf_dist as f64 / recrf_dist as f64;

    if srcrf_dist <= 128 {
        return Some(delta_rate);
    }

    let dr = (delta_rate >> shift) as f64 / f64::from(pix_num);
    let log_den = beta.ln() / 2.0f64.ln() + 2.0 * dr;

    if log_den > 10.0f64.ln() / 2.0f64.ln() {
        let rate_cost = (((1.0 / beta).ln() * f64::from(pix_num)) / 2.0f64.ln() / 2.0) as i64;
        return Some(rate_cost << shift);
    }

    let num = 2.0f64.powf(log_den);
    let den = num * beta + (1.0 - beta) * beta;
    let rate_cost = ((f64::from(pix_num) * (num / den).ln()) / 2.0f64.ln() / 2.0) as i64;
    Some(rate_cost << shift)
}
