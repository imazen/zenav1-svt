//! Leaves of `Source/Lib/Codec/noise_model.c` — the film-grain noise estimator
//! that `--film-grain-denoise` and `--fgs-table` drive.
//!
//! ## Scope
//!
//! Ported here: the two exported pointwise/window DSP kernels, the noise
//! strength solver's bin geometry and accumulation, `num_coeffs`, and the
//! flat-block score comparator.
//!
//! NOT ported here, named so this module is not read as "the noise model is
//! done" — it is a small fraction of a 45-function file:
//! * The AR model itself: `svt_aom_noise_model_init` / `_save_latest` /
//!   `_get_grain_parameters`, `noise_model_update`,
//!   `svt_av1_add_block_observations_internal_c`, `add_block_observations`,
//!   `add_noise_std_observations`, `ar_equation_system_solve`,
//!   `set_chroma_coefficient_fallback_soln`, `noise_state_init`.
//! * The flat-block finder: `svt_aom_flat_block_finder_init` / `_run` /
//!   `_extract_block_c`, `get_block_mean`, `get_noise_var`.
//! * The denoiser: `svt_aom_wiener_denoise_2d`, `svt_aom_denoise_and_model_run`,
//!   `unpack_2d_pic`, and `get_half_cos_window` — the last of which is six
//!   float tables totalling 5,460 literals, deliberately deferred because
//!   nothing that reads them is ported.
//! * The piecewise-linear fit: `svt_aom_noise_strength_solver_solve`,
//!   `_fit_piecewise`, `update_piecewise_linear_residual`.
//!   `equation_system_solve` is in that group too: it delegates to `linsolve`
//!   in `mathutils.h`, which is outside this file's surface.
//! * Allocation and lifecycle — `equation_system_init` / `_free` / `_clear` /
//!   `_copy`, `noise_strength_solver_clear` / `_copy`,
//!   `svt_aom_noise_strength_lut_init` / `_free`, `svt_aom_noise_model_free`,
//!   `svt_aom_flat_block_finder_free`, `svt_aom_denoise_and_model_ctor`,
//!   `denoise_and_model_dctor`, `denoise_and_model_realloc_if_necessary`.
//!   These are what an owned Rust type replaces rather than translates;
//!   [`NoiseStrengthSolver`] is that replacement for the solver's share of them.
//!
//! ## Evidence
//!
//! `pointwise_multiply`, `apply_window_function_to_plane`,
//! `noise_strength_solver_get_center` and `NoiseStrengthSolver::add_measurement`
//! are TIER 1 against the exported C symbols (`c_parity_noise_model.rs`).
//! `num_coeffs`, `bin_index`, `value_at` and `compare_scores` are `static` in C
//! with no exported caller that isolates them — TIER 4, with the C line cited.

use alloc::vec;
use alloc::vec::Vec;

/// `svt_av1_pointwise_multiply_c` (noise_model.c:1293). EXPORTED and
/// RTCD-dispatched — TIER 1.
///
/// `b[i] = a[i] * (float)b_d[i]` and likewise for `c`. The `(float)` casts are
/// load-bearing: each `f64` is narrowed BEFORE the multiply, so the product is
/// a single-precision one. Multiplying in `f64` and narrowing the result is a
/// different function.
pub fn pointwise_multiply(a: &[f32], b: &mut [f32], c: &mut [f32], b_d: &[f64], c_d: &[f64]) {
    let n = a
        .len()
        .min(b.len())
        .min(c.len())
        .min(b_d.len())
        .min(c_d.len());
    for i in 0..n {
        b[i] = a[i] * (b_d[i] as f32);
        c[i] = a[i] * (c_d[i] as f32);
    }
}

/// `svt_av1_apply_window_function_to_plane_c` (noise_model.c:2004). EXPORTED
/// and RTCD-dispatched — TIER 1.
///
/// ACCUMULATES `(block + plane) * window` into `result`; it does not assign.
/// `block`, `plane` and `window_function` are all packed at `x_size` while
/// `result` has its own `result_stride` — four buffers, three strides, and one
/// of them different from the others.
pub fn apply_window_function_to_plane(
    y_size: usize,
    x_size: usize,
    result: &mut [f32],
    result_stride: usize,
    block: &[f32],
    plane: &[f32],
    window_function: &[f32],
) {
    for y in 0..y_size {
        for x in 0..x_size {
            result[y * result_stride + x] +=
                (block[y * x_size + x] + plane[y * x_size + x]) * window_function[y * x_size + x];
        }
    }
}

/// The AR filter's support shape (`AomNoiseShape`, noise_model.h).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoiseShape {
    /// `AOM_NOISE_SHAPE_DIAMOND`.
    Diamond,
    /// `AOM_NOISE_SHAPE_SQUARE`.
    Square,
}

/// `num_coeffs` (noise_model.c:181). `static` in C — TIER 4.
///
/// How many AR coefficients a `(shape, lag)` support needs. C's default arm
/// returns 0 for an out-of-range `shape`; an enum makes that arm unreachable,
/// so it is gone rather than kept as dead code.
///
/// The square case is `(2*lag + 1)^2 / 2` with C's TRUNCATING division — the
/// support is the half-plane of a `(2 lag + 1)` square, and the centre tap is
/// excluded by that truncation rather than by a subtraction.
pub fn num_coeffs(shape: NoiseShape, lag: i32) -> i32 {
    let n = 2 * lag + 1;
    match shape {
        NoiseShape::Diamond => lag * (lag + 1),
        NoiseShape::Square => (n * n) / 2,
    }
}

/// `compare_scores` (noise_model.c:515). `static` in C — TIER 4.
///
/// C's `qsort` comparator over `IndexAndscore`, spelled
/// `diff < 0 ? -1 : diff > 0`. Note what that does NOT do: it never returns a
/// value derived from the index, so equal scores keep whatever order `qsort`
/// leaves them in — which is UNSPECIFIED, `qsort` not being stable. Reproducing
/// the ordering of a tie therefore needs the caller's sort to match C's
/// `qsort`, not just this comparator; a caller that uses Rust's stable
/// `sort_by` will differ from C wherever two blocks score identically.
///
/// The subtraction is in `f32` and its result is compared against zero, so a
/// NaN score makes BOTH tests false and the comparator answers EQUAL — C's
/// `diff < 0 ? -1 : diff > 0` yields `0` there, not `1`. (An earlier draft of
/// this comment said "greater"; corrected. Making the NaN case explicit is
/// byte-inert, mutation-confirmed, and left out for that reason.) It is also
/// why this returns `Ordering` from the same two tests rather than delegating
/// to `partial_cmp`, which would return `None`.
///
/// Two floats so close that their difference underflows to zero also compare
/// EQUAL, where a direct `<` would separate them. That is C's, and it is the
/// second reason the subtraction is kept.
pub fn compare_scores(a: f32, b: f32) -> core::cmp::Ordering {
    let diff = a - b;
    if diff < 0.0 {
        core::cmp::Ordering::Less
    } else if diff > 0.0 {
        core::cmp::Ordering::Greater
    } else {
        core::cmp::Ordering::Equal
    }
}

/// `AomNoiseStrengthSolver` (noise_model.h:69) — noise standard deviation as a
/// piecewise-linear function of block intensity, accumulated as a normal
/// equation system over `num_bins` bins.
///
/// C's `AomEquationSystem` is three `malloc`ed arrays plus a size, with
/// `equation_system_init` / `_free` / `_clear` / `_copy` maintaining them; the
/// owned `Vec`s here replace that group rather than translating it, which is
/// why those five functions are counted out in the module header.
#[derive(Debug, Clone, PartialEq)]
pub struct NoiseStrengthSolver {
    /// `eqns.A`, row-major `num_bins x num_bins`.
    pub a: Vec<f64>,
    /// `eqns.b`.
    pub b: Vec<f64>,
    /// `eqns.x` — the solution, untouched by this module.
    pub x: Vec<f64>,
    pub min_intensity: f64,
    pub max_intensity: f64,
    pub num_bins: i32,
    pub num_equations: i32,
    pub total: f64,
}

impl NoiseStrengthSolver {
    /// `svt_aom_noise_strength_solver_init` (noise_model.c:305).
    ///
    /// `max_intensity` is `(1 << bit_depth) - 1` computed in `int` and stored
    /// as a `double`, so it is exact for every bit depth the encoder accepts.
    pub fn new(num_bins: i32, bit_depth: u32) -> Self {
        let n = num_bins.max(0) as usize;
        Self {
            a: vec![0.0; n * n],
            b: vec![0.0; n],
            x: vec![0.0; n],
            min_intensity: 0.0,
            max_intensity: f64::from((1i32 << bit_depth) - 1),
            num_bins,
            num_equations: 0,
            total: 0.0,
        }
    }

    /// `noise_strength_solver_get_bin_index` (noise_model.c:236). `static` in
    /// C — TIER 4.
    ///
    /// A FRACTIONAL bin position, not an index: the caller floors it and
    /// interpolates between the two neighbours. `value` is clamped to
    /// `[min_intensity, max_intensity]` first, so the result is always inside
    /// `[0, num_bins - 1]`.
    pub fn bin_index(&self, value: f64) -> f64 {
        let val = value.clamp(self.min_intensity, self.max_intensity);
        let range = self.max_intensity - self.min_intensity;
        f64::from(self.num_bins - 1) * (val - self.min_intensity) / range
    }

    /// `svt_aom_noise_strength_solver_get_center` (noise_model.c:318).
    /// EXPORTED — TIER 1.
    ///
    /// The intensity at the centre of bin `i`. Not the inverse of
    /// [`Self::bin_index`] outside `0..num_bins`, because it does not clamp.
    pub fn get_center(&self, i: i32) -> f64 {
        let range = self.max_intensity - self.min_intensity;
        f64::from(i) / f64::from(self.num_bins - 1) * range + self.min_intensity
    }

    /// `noise_strength_solver_get_value` (noise_model.c:242). `static` in C —
    /// TIER 4.
    ///
    /// Linear interpolation of the SOLVED curve at intensity `x`. Reads
    /// `eqns.x`, so it is meaningless before the system is solved — and this
    /// module does not solve it (see the header).
    pub fn value_at(&self, x: f64) -> f64 {
        let bin = self.bin_index(x);
        let bin_i0 = bin.floor() as i32;
        let bin_i1 = (self.num_bins - 1).min(bin_i0 + 1);
        let a = bin - f64::from(bin_i0);
        (1.0 - a) * self.x[bin_i0 as usize] + a * self.x[bin_i1 as usize]
    }

    /// `svt_aom_noise_strength_solver_add_measurement` (noise_model.c:250).
    /// EXPORTED — TIER 1.
    ///
    /// Accumulates one `(block_mean, noise_std)` observation into the normal
    /// equations, spread over the two bins the mean falls between.
    ///
    /// `A[i1][i0]` and `A[i0][i1]` are BOTH incremented by `a * (1 - a)` as
    /// separate statements, and `b[i0]` / `b[i1]` likewise. When
    /// `bin_i0 == bin_i1` each pair addresses the SAME cell twice.
    ///
    /// CORRECTED by mutation, 2026-08-31: this comment first claimed that
    /// collapsing the pairs into one update would halve the doubled cell.
    /// It would not, and the differential proved it — guarding either second
    /// update with `if i0 != i1` is byte-inert. `bin_i0 == bin_i1` happens only
    /// at the very top, where `bin_index` clamps `val` to `max_intensity` and
    /// `bin` comes out as exactly `num_bins - 1` (the division is of two equal
    /// `f64`s, so it is exactly 1.0). There `a == 0`, and both of the doubled
    /// updates add zero. The one that is NOT inert is `A[i1][i0]`, which the
    /// differential does catch — it carries the real cross term everywhere in
    /// the interior.
    ///
    /// The statements are still kept one-to-one with C: the reasoning above
    /// depends on `bin_index`'s exact clamp, and a future change there would
    /// make the collapse wrong again with nothing to notice it.
    pub fn add_measurement(&mut self, block_mean: f64, noise_std: f64) {
        let bin = self.bin_index(block_mean);
        let bin_i0 = bin.floor() as i32;
        let bin_i1 = (self.num_bins - 1).min(bin_i0 + 1);
        let a = bin - f64::from(bin_i0);
        let n = self.num_bins as usize;
        let (i0, i1) = (bin_i0 as usize, bin_i1 as usize);

        self.a[i0 * n + i0] += (1.0 - a) * (1.0 - a);
        self.a[i1 * n + i0] += a * (1.0 - a);
        self.a[i1 * n + i1] += a * a;
        self.a[i0 * n + i1] += a * (1.0 - a);
        self.b[i0] += (1.0 - a) * noise_std;
        self.b[i1] += a * noise_std;
        self.total += noise_std;
        self.num_equations += 1;
    }
}
