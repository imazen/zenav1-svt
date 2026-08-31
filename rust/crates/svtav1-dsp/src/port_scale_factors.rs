//! Reference scale factors, the MC scale dispatch, and the compound distance
//! weights.
//!
//! Ported from `Source/Lib/Codec/inter_prediction.c` (SVT-AV1 v4.2.0):
//! `svt_av1_setup_scale_factors_for_frame` (:201),
//! `get_fixed_point_scale_factor` (:188),
//! `fixed_point_scale_to_coarse_point_scale` (:197), `scaled_x` (:169),
//! `scaled_y` (:176), `unscaled_value` (:183), `has_scale` (:223),
//! `revert_scale_extra_bits` (:227), `svt_aom_get_relative_dist_enc` (:274)
//! and `svt_av1_dist_wtd_comp_weight_assign` (:290); plus the header inlines
//! `valid_ref_frame_size`, `av1_is_valid_scale` and `av1_is_scaled`
//! (inter_prediction.h:161-172).
//!
//! # This is NOT [`crate::scale`]
//!
//! `scale.rs::ScaleFactors::new` is a homegrown `(ref << 14) / cur` divide with
//! a Q14 `scale_x` / `scale_y`. C's factor is
//! `((other << REF_SCALE_SHIFT) + this/2) / this` — round-half-up, not
//! truncate — and its `scaled_x` carries a `(x_scale_fp - (1 << 14)) << 3`
//! offset and rounds a 64-bit product down `REF_SCALE_SHIFT - SCALE_EXTRA_BITS`
//! bits into the SCALE_SUBPEL (10-bit) domain, not Q14. The two disagree at
//! every non-unity scale. `scale.rs` is left alone here — `c_parity_scale.rs`
//! pins its divergence deliberately.

/// `REF_SCALE_SHIFT` (inter_prediction.h:27).
pub const REF_SCALE_SHIFT: i32 = 14;
/// `REF_NO_SCALE` (inter_prediction.h:28).
pub const REF_NO_SCALE: i32 = 1 << REF_SCALE_SHIFT;
/// `REF_INVALID_SCALE` (inter_prediction.h:29).
pub const REF_INVALID_SCALE: i32 = -1;
/// `SUBPEL_BITS` (definitions.h:457).
pub const SUBPEL_BITS: i32 = 4;
/// `SUBPEL_SHIFTS` (definitions.h:459).
pub const SUBPEL_SHIFTS: i32 = 1 << SUBPEL_BITS;
/// `SCALE_SUBPEL_BITS` (definitions.h:462).
pub const SCALE_SUBPEL_BITS: i32 = 10;
/// `SCALE_SUBPEL_SHIFTS` (definitions.h:463).
pub const SCALE_SUBPEL_SHIFTS: i32 = 1 << SCALE_SUBPEL_BITS;
/// `SCALE_EXTRA_BITS` (definitions.h:465).
pub const SCALE_EXTRA_BITS: i32 = SCALE_SUBPEL_BITS - SUBPEL_BITS;
/// `MAX_FRAME_DISTANCE` — the clamp on the two reference distances.
pub const MAX_FRAME_DISTANCE: i32 = 31;

/// `ScaleFactors` (definitions.h:1778) minus the two function pointers, which
/// only select between [`scaled_x`]/[`scaled_y`] and [`unscaled_value`] —
/// a decision [`ScaleFactors::is_scaled`] reproduces directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScaleFactors {
    /// `x_scale_fp` — horizontal fixed-point scale, or `REF_INVALID_SCALE`.
    pub x_scale_fp: i32,
    /// `y_scale_fp`.
    pub y_scale_fp: i32,
    /// `x_step_q4` — the coarse (SCALE_SUBPEL) step.
    pub x_step_q4: i32,
    /// `y_step_q4`.
    pub y_step_q4: i32,
}

/// `valid_ref_frame_size` (inter_prediction.h:169) — AV1 permits a reference
/// at most 2x larger and at most 16x smaller in each dimension.
pub fn valid_ref_frame_size(
    ref_width: i32,
    ref_height: i32,
    this_width: i32,
    this_height: i32,
) -> bool {
    2 * this_width >= ref_width
        && 2 * this_height >= ref_height
        && this_width <= 16 * ref_width
        && this_height <= 16 * ref_height
}

/// `get_fixed_point_scale_factor` (inter_prediction.c:188).
///
/// `((other << REF_SCALE_SHIFT) + this/2) / this` — round-half-up. A plain
/// truncating divide (what `scale.rs` does) differs by one at most sizes.
pub fn get_fixed_point_scale_factor(other_size: i32, this_size: i32) -> i32 {
    // C computes this entirely in `int32_t`; the shift is reproduced with
    // wrapping semantics rather than widened, so a large `other_size` folds
    // the same way it does in C rather than diverging.
    (other_size
        .wrapping_shl(REF_SCALE_SHIFT as u32)
        .wrapping_add(this_size / 2))
        / this_size
}

/// `fixed_point_scale_to_coarse_point_scale` (inter_prediction.c:197).
pub fn fixed_point_scale_to_coarse_point_scale(scale_fp: i32) -> i32 {
    round_power_of_two(scale_fp, REF_SCALE_SHIFT - SCALE_SUBPEL_BITS)
}

#[inline]
fn round_power_of_two(value: i32, n: i32) -> i32 {
    if n == 0 {
        value
    } else {
        (value + (1 << (n - 1))) >> n
    }
}

/// `ROUND_POWER_OF_TWO_SIGNED_64` — round-half-away-from-zero on an i64.
#[inline]
fn round_power_of_two_signed_64(value: i64, n: i32) -> i64 {
    if value < 0 {
        -round_power_of_two_64(-value, n)
    } else {
        round_power_of_two_64(value, n)
    }
}

#[inline]
fn round_power_of_two_64(value: i64, n: i32) -> i64 {
    if n == 0 {
        value
    } else {
        (value + (1i64 << (n - 1))) >> n
    }
}

impl ScaleFactors {
    /// `svt_av1_setup_scale_factors_for_frame` (inter_prediction.c:201).
    ///
    /// On an invalid reference size C sets both `*_scale_fp` to
    /// `REF_INVALID_SCALE` and **returns without touching `*_step_q4`** — the
    /// caller's struct keeps whatever was there. That early return is
    /// reproduced by leaving the steps at the `sentinel` the caller supplies.
    pub fn setup_for_frame_with_sentinel(
        other_w: i32,
        other_h: i32,
        this_w: i32,
        this_h: i32,
        sentinel: i32,
    ) -> Self {
        if !valid_ref_frame_size(other_w, other_h, this_w, this_h) {
            return Self {
                x_scale_fp: REF_INVALID_SCALE,
                y_scale_fp: REF_INVALID_SCALE,
                x_step_q4: sentinel,
                y_step_q4: sentinel,
            };
        }
        let x_scale_fp = get_fixed_point_scale_factor(other_w, this_w);
        let y_scale_fp = get_fixed_point_scale_factor(other_h, this_h);
        Self {
            x_scale_fp,
            y_scale_fp,
            x_step_q4: fixed_point_scale_to_coarse_point_scale(x_scale_fp),
            y_step_q4: fixed_point_scale_to_coarse_point_scale(y_scale_fp),
        }
    }

    /// `svt_av1_setup_scale_factors_for_frame` on a zeroed struct.
    pub fn setup_for_frame(other_w: i32, other_h: i32, this_w: i32, this_h: i32) -> Self {
        Self::setup_for_frame_with_sentinel(other_w, other_h, this_w, this_h, 0)
    }

    /// `av1_is_valid_scale` (inter_prediction.h:161).
    pub fn is_valid_scale(&self) -> bool {
        self.x_scale_fp != REF_INVALID_SCALE && self.y_scale_fp != REF_INVALID_SCALE
    }

    /// `av1_is_scaled` (inter_prediction.h:165).
    pub fn is_scaled(&self) -> bool {
        self.is_valid_scale()
            && (self.x_scale_fp != REF_NO_SCALE || self.y_scale_fp != REF_NO_SCALE)
    }

    /// `scaled_x` (inter_prediction.c:169). `val` is in q4 precision; the
    /// result is in the SCALE_SUBPEL (10-bit) domain.
    pub fn scaled_x(&self, val: i32) -> i32 {
        let off = (self.x_scale_fp - (1 << REF_SCALE_SHIFT)) * (1 << (SUBPEL_BITS - 1));
        let tval = val as i64 * self.x_scale_fp as i64 + off as i64;
        round_power_of_two_signed_64(tval, REF_SCALE_SHIFT - SCALE_EXTRA_BITS) as i32
    }

    /// `scaled_y` (inter_prediction.c:176).
    pub fn scaled_y(&self, val: i32) -> i32 {
        let off = (self.y_scale_fp - (1 << REF_SCALE_SHIFT)) * (1 << (SUBPEL_BITS - 1));
        let tval = val as i64 * self.y_scale_fp as i64 + off as i64;
        round_power_of_two_signed_64(tval, REF_SCALE_SHIFT - SCALE_EXTRA_BITS) as i32
    }

    /// The `scale_value_x` / `scale_value_y` function pointers C installs:
    /// [`Self::scaled_x`] when scaled, [`unscaled_value`] otherwise.
    pub fn scale_value_x(&self, val: i32) -> i32 {
        if self.is_scaled() {
            self.scaled_x(val)
        } else {
            unscaled_value(val)
        }
    }

    /// Vertical counterpart of [`Self::scale_value_x`].
    pub fn scale_value_y(&self, val: i32) -> i32 {
        if self.is_scaled() {
            self.scaled_y(val)
        } else {
            unscaled_value(val)
        }
    }
}

/// `unscaled_value` (inter_prediction.c:183) — the identity path's promotion
/// from q4 into the SCALE_SUBPEL domain. Taken on the NON-scaled branch, so it
/// is not superres-only.
pub fn unscaled_value(val: i32) -> i32 {
    val << SCALE_EXTRA_BITS
}

/// `SubpelParams` (inter_prediction.h:49).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SubpelParams {
    /// Horizontal step in the SCALE_SUBPEL domain.
    pub xs: i32,
    /// Vertical step.
    pub ys: i32,
    /// Horizontal sub-pel phase.
    pub subpel_x: i32,
    /// Vertical sub-pel phase.
    pub subpel_y: i32,
}

/// `has_scale` (inter_prediction.c:223) — the first branch of every MC entry
/// point (`svt_inter_predictor_pd0`, `_light_pd1`, `svt_inter_predictor`,
/// `svt_highbd_inter_predictor`, `enc_make_inter_predictor`).
pub fn has_scale(xs: i32, ys: i32) -> bool {
    xs != SCALE_SUBPEL_SHIFTS || ys != SCALE_SUBPEL_SHIFTS
}

/// `revert_scale_extra_bits` (inter_prediction.c:227) — strips
/// `SCALE_EXTRA_BITS` before the non-scaled kernels are called. Reached on the
/// ORDINARY unscaled path, so it is not superres-only.
///
/// C asserts `subpel_* < SUBPEL_SHIFTS` and `*s <= SUBPEL_SHIFTS` afterwards;
/// those are debug assertions on the caller's contract, kept as `debug_assert`.
pub fn revert_scale_extra_bits(sp: &mut SubpelParams) {
    sp.subpel_x >>= SCALE_EXTRA_BITS;
    sp.subpel_y >>= SCALE_EXTRA_BITS;
    sp.xs >>= SCALE_EXTRA_BITS;
    sp.ys >>= SCALE_EXTRA_BITS;
    debug_assert!(sp.subpel_x < SUBPEL_SHIFTS);
    debug_assert!(sp.subpel_y < SUBPEL_SHIFTS);
    debug_assert!(sp.xs <= SUBPEL_SHIFTS);
    debug_assert!(sp.ys <= SUBPEL_SHIFTS);
}

/// `svt_aom_get_relative_dist_enc` (inter_prediction.c:274) — the signed
/// order-hint difference, wrapped into `[-m, m)` with `m = 1 << (bits - 1)`.
///
/// Returns 0 when order hints are disabled, whatever the arguments.
pub fn get_relative_dist_enc(
    enable_order_hint: bool,
    order_hint_bits: i32,
    ref_hint: i32,
    order_hint: i32,
) -> i32 {
    if !enable_order_hint {
        return 0;
    }
    let mut diff = ref_hint - order_hint;
    let m = 1 << (order_hint_bits - 1);
    diff = (diff & (m - 1)) - (diff & m);
    diff
}

/// `quant_dist_weight` (inter_prediction.c:284).
const QUANT_DIST_WEIGHT: [[i32; 2]; 4] = [[2, 3], [2, 5], [2, 7], [1, MAX_FRAME_DISTANCE]];

/// `quant_dist_lookup_table` (inter_prediction.c:285).
const QUANT_DIST_LOOKUP_TABLE: [[[i32; 2]; 4]; 2] = [
    [[9, 7], [11, 5], [12, 4], [13, 3]],
    [[7, 9], [5, 11], [4, 12], [3, 13]],
];

/// The `svt_av1_dist_wtd_comp_weight_assign` outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DistWtdWeights {
    /// `*fwd_offset`.
    pub fwd_offset: i32,
    /// `*bck_offset`.
    pub bck_offset: i32,
    /// `*use_dist_wtd_comp_avg`.
    pub use_dist_wtd_comp_avg: i32,
}

/// `svt_av1_dist_wtd_comp_weight_assign` (inter_prediction.c:290).
///
/// TRAP, reproduced: on the `!is_compound || compound_idx` early return C sets
/// **only** `*use_dist_wtd_comp_avg = 0` and leaves `*fwd_offset` /
/// `*bck_offset` at whatever the caller had. `prev` carries those in so the
/// early return is observable rather than silently zeroed.
///
/// `order = d0 <= d1` is a BOOLEAN used as the second index of
/// `quant_dist_weight[i][order]` / `quant_dist_lookup_table[order_idx][i][order]`
/// — not a distance. Reading it as anything else transposes the table.
pub fn dist_wtd_comp_weight_assign(
    enable_order_hint: bool,
    order_hint_bits: i32,
    cur_frame_index: i32,
    bck_frame_index: i32,
    fwd_frame_index: i32,
    compound_idx: i32,
    order_idx: usize,
    is_compound: bool,
    prev: DistWtdWeights,
) -> DistWtdWeights {
    if !is_compound || compound_idx != 0 {
        return DistWtdWeights {
            use_dist_wtd_comp_avg: 0,
            ..prev
        };
    }

    let d0 = get_relative_dist_enc(
        enable_order_hint,
        order_hint_bits,
        fwd_frame_index,
        cur_frame_index,
    )
    .abs()
    .clamp(0, MAX_FRAME_DISTANCE);
    let d1 = get_relative_dist_enc(
        enable_order_hint,
        order_hint_bits,
        cur_frame_index,
        bck_frame_index,
    )
    .abs()
    .clamp(0, MAX_FRAME_DISTANCE);

    let order = usize::from(d0 <= d1);

    if d0 == 0 || d1 == 0 {
        return DistWtdWeights {
            fwd_offset: QUANT_DIST_LOOKUP_TABLE[order_idx][3][order],
            bck_offset: QUANT_DIST_LOOKUP_TABLE[order_idx][3][1 - order],
            use_dist_wtd_comp_avg: 1,
        };
    }

    // C's loop leaves `i == 3` when nothing breaks — the same index the
    // `d0 == 0 || d1 == 0` arm uses.
    let mut i = 3usize;
    for cand in 0..3usize {
        let c0 = QUANT_DIST_WEIGHT[cand][order];
        let c1 = QUANT_DIST_WEIGHT[cand][1 - order];
        let d0_c0 = d0 * c0;
        let d1_c1 = d1 * c1;
        if (d0 > d1 && d0_c0 < d1_c1) || (d0 <= d1 && d0_c0 > d1_c1) {
            i = cand;
            break;
        }
    }

    DistWtdWeights {
        fwd_offset: QUANT_DIST_LOOKUP_TABLE[order_idx][i][order],
        bck_offset: QUANT_DIST_LOOKUP_TABLE[order_idx][i][1 - order],
        use_dist_wtd_comp_avg: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1:1 reference is valid and NOT scaled, and its step is exactly
    /// `SCALE_SUBPEL_SHIFTS` — the value `has_scale` compares against.
    #[test]
    fn unity_scale_is_not_scaled() {
        let sf = ScaleFactors::setup_for_frame(640, 480, 640, 480);
        assert_eq!(sf.x_scale_fp, REF_NO_SCALE);
        assert_eq!(sf.y_scale_fp, REF_NO_SCALE);
        assert_eq!(sf.x_step_q4, SCALE_SUBPEL_SHIFTS);
        assert_eq!(sf.y_step_q4, SCALE_SUBPEL_SHIFTS);
        assert!(sf.is_valid_scale());
        assert!(!sf.is_scaled());
        assert!(!has_scale(sf.x_step_q4, sf.y_step_q4));
    }

    /// The size limits: 2x larger and 16x smaller, inclusive.
    #[test]
    fn valid_ref_frame_size_bounds() {
        assert!(valid_ref_frame_size(128, 128, 64, 64)); // exactly 2x larger
        assert!(!valid_ref_frame_size(130, 128, 64, 64));
        assert!(valid_ref_frame_size(64, 64, 1024, 1024)); // exactly 16x smaller
        assert!(!valid_ref_frame_size(64, 64, 1040, 1024));
        let bad = ScaleFactors::setup_for_frame(130, 128, 64, 64);
        assert_eq!(bad.x_scale_fp, REF_INVALID_SCALE);
        assert!(!bad.is_valid_scale());
        assert!(!bad.is_scaled());
    }

    /// `unscaled_value` and `revert_scale_extra_bits` are inverses on the
    /// identity path, which is why the unscaled branch reaches both.
    #[test]
    fn revert_undoes_unscaled_promotion() {
        let mut sp = SubpelParams {
            xs: unscaled_value(SUBPEL_SHIFTS),
            ys: unscaled_value(SUBPEL_SHIFTS),
            subpel_x: unscaled_value(9),
            subpel_y: unscaled_value(3),
        };
        revert_scale_extra_bits(&mut sp);
        assert_eq!(
            sp,
            SubpelParams {
                xs: SUBPEL_SHIFTS,
                ys: SUBPEL_SHIFTS,
                subpel_x: 9,
                subpel_y: 3
            }
        );
    }

    /// The `!is_compound` early return leaves fwd/bck untouched.
    #[test]
    fn non_compound_leaves_offsets_alone() {
        let prev = DistWtdWeights {
            fwd_offset: 111,
            bck_offset: 222,
            use_dist_wtd_comp_avg: 1,
        };
        let got = dist_wtd_comp_weight_assign(true, 7, 4, 8, 2, 0, 0, false, prev);
        assert_eq!(got.fwd_offset, 111);
        assert_eq!(got.bck_offset, 222);
        assert_eq!(got.use_dist_wtd_comp_avg, 0);
        // compound_idx != 0 takes the same arm.
        let got = dist_wtd_comp_weight_assign(true, 7, 4, 8, 2, 1, 0, true, prev);
        assert_eq!(
            (got.fwd_offset, got.bck_offset, got.use_dist_wtd_comp_avg),
            (111, 222, 0)
        );
    }
}
