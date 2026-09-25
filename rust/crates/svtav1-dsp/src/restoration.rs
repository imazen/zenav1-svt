//! Loop restoration — C-exact Wiener ports (kernel, statistics, tap solver,
//! decoder-exact per-unit stripe filtering).
//!
//! Sources (SVT-AV1 v4.2.0-rc, byte-identical to libaom's restoration for
//! this scope):
//! - Kernel: `svt_av1_wiener_convolve_add_src_c` (convolve.c:106) —
//!   horizontal pass `svt_aom_convolve_add_src_horiz_hip` then vertical
//!   `svt_aom_convolve_add_src_vert_hip`. The InterpKernel base/offset
//!   pointer arithmetic in the C entry cancels exactly (x_step_q4 = 16 and
//!   16-aligned filter storage mean every output pixel uses the SAME 8-tap
//!   filter and the source index advances by 1), so the port consumes the 8
//!   taps directly.
//! - Statistics: `svt_av1_compute_stats_c` + `find_average`
//!   (restoration_pick.c:652, restoration_pick.h:21).
//! - Solver: `linsolve_wiener` / `update_a_sep_sym` / `update_b_sep_sym` /
//!   `wiener_decompose_sep_sym` / `finalize_sym_filter` / `compute_score`
//!   (restoration_pick.c:745-1003).
//! - Unit filter: `svt_av1_loop_restoration_filter_unit` +
//!   `get_stripe_boundary_info` + `{setup,restore}_processing_stripe_boundary`
//!   + `wiener_filter_stripe` (restoration.c:216-421, 1040-1110), the
//!     decoder-authoritative stripe walk (libaom av1/common/restoration.c is
//!     the same code).
//! - Boundary capture: `svt_aom_save_deblock_boundary_lines` /
//!   `svt_aom_save_cdef_boundary_lines` / `svt_aom_save_tile_row_boundary_lines`
//!   (restoration.c:1507-1662) — the two-pass (post-deblock, post-CDEF)
//!   line-buffer scheme, single-tile form.
//!
//! Every function here is differentially fuzzed against the C archive in
//! `tests/c_parity_wiener.rs`.

#[allow(unused_imports)]
use archmage::prelude::*;

/// WIENER_WIN (7-tap window) — restoration.h:116.
pub const WIENER_WIN: usize = 7;
/// WIENER_WIN_CHROMA (5-tap window) — restoration.h:123.
pub const WIENER_WIN_CHROMA: usize = 5;
/// WIENER_HALFWIN — restoration.h:45.
pub const WIENER_HALFWIN: usize = 3;
/// WIENER_FILT_STEP = 1 << WIENER_FILT_PREC_BITS(7) — restoration.h:126.
pub const WIENER_FILT_STEP: i32 = 128;

/// Central tap values (restoration.h:129-133).
pub const WIENER_FILT_TAP0_MIDV: i32 = 3;
pub const WIENER_FILT_TAP1_MIDV: i32 = -7;
pub const WIENER_FILT_TAP2_MIDV: i32 = 15;

/// Tap bit budgets (restoration.h:135-137).
pub const WIENER_FILT_TAP0_BITS: i32 = 4;
pub const WIENER_FILT_TAP1_BITS: i32 = 5;
pub const WIENER_FILT_TAP2_BITS: i32 = 6;

/// Tap min/max bounds (restoration.h:141-147).
pub const WIENER_FILT_TAP0_MINV: i32 = WIENER_FILT_TAP0_MIDV - (1 << WIENER_FILT_TAP0_BITS) / 2;
pub const WIENER_FILT_TAP1_MINV: i32 = WIENER_FILT_TAP1_MIDV - (1 << WIENER_FILT_TAP1_BITS) / 2;
pub const WIENER_FILT_TAP2_MINV: i32 = WIENER_FILT_TAP2_MIDV - (1 << WIENER_FILT_TAP2_BITS) / 2;
pub const WIENER_FILT_TAP0_MAXV: i32 = WIENER_FILT_TAP0_MIDV - 1 + (1 << WIENER_FILT_TAP0_BITS) / 2;
pub const WIENER_FILT_TAP1_MAXV: i32 = WIENER_FILT_TAP1_MIDV - 1 + (1 << WIENER_FILT_TAP1_BITS) / 2;
pub const WIENER_FILT_TAP2_MAXV: i32 = WIENER_FILT_TAP2_MIDV - 1 + (1 << WIENER_FILT_TAP2_BITS) / 2;

/// Subexp K parameters for tap coding (restoration.h:149-151).
pub const WIENER_FILT_TAP0_SUBEXP_K: u16 = 1;
pub const WIENER_FILT_TAP1_SUBEXP_K: u16 = 2;
pub const WIENER_FILT_TAP2_SUBEXP_K: u16 = 3;

/// RESTORATION_PROC_UNIT_SIZE — restoration.h:36.
pub const RESTORATION_PROC_UNIT_SIZE: i32 = 64;
/// RESTORATION_UNIT_OFFSET — restoration.h:39.
pub const RESTORATION_UNIT_OFFSET: i32 = 8;
/// RESTORATION_BORDER (context pixels per processing unit) — restoration.h:64.
pub const RESTORATION_BORDER: i32 = 3;
/// RESTORATION_CTX_VERT (saved deblock rows per stripe edge) — restoration.h:68.
pub const RESTORATION_CTX_VERT: i32 = 2;
/// RESTORATION_EXTRA_HORZ — restoration.h:72.
pub const RESTORATION_EXTRA_HORZ: i32 = 4;
/// RESTORATION_UNITSIZE_MAX — restoration.h:80.
pub const RESTORATION_UNITSIZE_MAX: i32 = 256;

/// `WIENER_ROUND0_BITS` (convolve.h:24) for 8-bit.
pub const WIENER_ROUND0_BITS: i32 = 3;
/// `FILTER_BITS` (definitions.h:442).
pub const FILTER_BITS: i32 = 7;
/// 2 * FILTER_BITS - round0 (get_conv_params_wiener, convolve.h:79).
pub const WIENER_ROUND1_BITS: i32 = 2 * FILTER_BITS - WIENER_ROUND0_BITS;

/// RestorationType values (matches C enum order: av1_structs.h).
pub const RESTORE_NONE: u8 = 0;
pub const RESTORE_WIENER: u8 = 1;
pub const RESTORE_SGRPROJ: u8 = 2;
pub const RESTORE_SWITCHABLE: u8 = 3;

/// C `WienerInfo` (restoration.h:167): 8-element InterpKernels; tap\[7\] is
/// always 0 (the kernel runs 8 taps with the last weight zero).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WienerInfo {
    pub vfilter: [i16; 8],
    pub hfilter: [i16; 8],
}

/// C `RestorationUnitInfo` (restoration.h:206) — the per-unit filter choice
/// the stripe walk dispatches on. C passes this one struct to
/// `svt_av1_loop_restoration_filter_unit`; the port used to pass
/// `(rtype, &WienerInfo)` because sgrproj was unreachable on the all-intra
/// path (`sg_filter_lvl = 0` at every representable preset). It is reachable
/// in VIDEO mode at presets 0..3 (`svt_aom_get_sg_filter_level_default`,
/// enc_mode_config.c:1402), so the SGR arm now travels with the Wiener one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RestUnitParams {
    /// `RESTORE_NONE` / `RESTORE_WIENER` / `RESTORE_SGRPROJ`.
    pub rtype: u8,
    pub wiener: WienerInfo,
    /// `SgrprojInfo::ep` — the `SGR_PARAMS` index.
    pub sgr_ep: usize,
    /// `SgrprojInfo::xqd`.
    pub sgr_xqd: [i32; 2],
}

impl RestUnitParams {
    /// A NONE unit (no filter); the two filter payloads are the C defaults.
    #[must_use]
    pub fn none() -> Self {
        RestUnitParams {
            rtype: RESTORE_NONE,
            wiener: WienerInfo::default(),
            sgr_ep: 0,
            sgr_xqd: DEFAULT_SGRPROJ_XQD,
        }
    }

    /// A Wiener unit with these taps.
    #[must_use]
    pub fn wiener(wiener: WienerInfo) -> Self {
        RestUnitParams {
            rtype: RESTORE_WIENER,
            wiener,
            sgr_ep: 0,
            sgr_xqd: DEFAULT_SGRPROJ_XQD,
        }
    }

    /// An SGR unit with this `(ep, xqd)`.
    #[must_use]
    pub fn sgrproj(ep: usize, xqd: [i32; 2]) -> Self {
        RestUnitParams {
            rtype: RESTORE_SGRPROJ,
            wiener: WienerInfo::default(),
            sgr_ep: ep,
            sgr_xqd: xqd,
        }
    }
}

/// C `set_default_sgrproj` (restoration.h:243): the midpoint of each `xqd`
/// range. This is the value both the SEARCH reference (`rsc_on_tile`,
/// restoration_pick.c:89) and the WRITER reference
/// (`svt_av1_reset_loop_restoration`, entropy_coding.c:4019) start from, so
/// the first coded `refsubexpfin` delta is measured against it.
pub const DEFAULT_SGRPROJ_XQD: [i32; 2] = [
    (crate::port_sgr::SGRPROJ_PRJ_MIN0 + crate::port_sgr::SGRPROJ_PRJ_MAX0) / 2,
    (crate::port_sgr::SGRPROJ_PRJ_MIN1 + crate::port_sgr::SGRPROJ_PRJ_MAX1) / 2,
];

impl Default for WienerInfo {
    /// C `set_default_wiener` (restoration.h:248): the mid taps.
    fn default() -> Self {
        let mid = [
            WIENER_FILT_TAP0_MIDV as i16,
            WIENER_FILT_TAP1_MIDV as i16,
            WIENER_FILT_TAP2_MIDV as i16,
            (-2 * (WIENER_FILT_TAP2_MIDV + WIENER_FILT_TAP1_MIDV + WIENER_FILT_TAP0_MIDV)) as i16,
            WIENER_FILT_TAP2_MIDV as i16,
            WIENER_FILT_TAP1_MIDV as i16,
            WIENER_FILT_TAP0_MIDV as i16,
            0,
        ];
        WienerInfo {
            vfilter: mid,
            hfilter: mid,
        }
    }
}

use svtav1_types::math::shift_i32::round_power_of_two_i32 as round_power_of_two;

/// The widest processing unit `wiener_filter_stripe` can ask for:
/// `w = procunit_width.min(..)` and `procunit_width = RESTORATION_PROC_UNIT_SIZE
/// >> ss_x`, so luma is 64 and chroma narrower. The streamed path below sizes
/// its row ring to this; anything wider falls back to
/// [`wiener_convolve_add_src_materialised`] rather than panicking.
const WIENER_MAX_PROC_W: usize = RESTORATION_PROC_UNIT_SIZE as usize;

/// One horizontal-pass row of `svt_aom_convolve_add_src_horiz_hip`.
///
/// `s` starts at the row's `x = -3` sample, so the 8-tap window for output `x`
/// is `s[x .. x + 8]` and the centre tap is `s[x + 3]`.
fn wiener_h_row(s: &[u8], w: usize, f: &[i16; 8], clamp_limit: i32, out: &mut [u16]) {
    for x in 0..w {
        let mut sum: i32 = (i32::from(s[x + 3]) << FILTER_BITS) + (1 << (8 + FILTER_BITS - 1));
        for (k, &fk) in f.iter().enumerate() {
            sum += i32::from(s[x + k]) * i32::from(fk);
        }
        out[x] = round_power_of_two(sum, WIENER_ROUND0_BITS).clamp(0, clamp_limit) as u16;
    }
}

/// One vertical-pass row of `svt_aom_convolve_add_src_vert_hip`.
///
/// `rows[k]` is horizontal-pass row `y + k`; the centre tap is `rows[3]`.
fn wiener_v_row(rows: &[&[u16]; 8], w: usize, f: &[i16; 8], out: &mut [u8]) {
    for x in 0..w {
        let mut sum: i32 =
            (i32::from(rows[3][x]) << FILTER_BITS) - (1 << (8 + WIENER_ROUND1_BITS - 1));
        for (k, &fk) in f.iter().enumerate() {
            sum += i32::from(rows[k][x]) * i32::from(fk);
        }
        out[x] = round_power_of_two(sum, WIENER_ROUND1_BITS).clamp(0, 255) as u8;
    }
}

/// Ring-row width for the vector arm: a multiple of 16 (the `i32x16` lane
/// count) that covers `WIENER_MAX_PROC_W`. Lanes past `w` hold padding and are
/// never stored to `dst`.
const WIENER_SIMD_RING_W: usize = 64;

/// Widened-source scratch width for the vector arm. The horizontal window for
/// output `x` reads `s32[x .. x + 8]`, and the last vector block starts at
/// `x = 48` (for `w = 49..64`), so the highest index touched is
/// `48 + 7 + 15 = 70`; 96 covers it with room for the zero tail.
const WIENER_SIMD_SRC_W: usize = 96;

/// Whether [`wiener_convolve_simd`] is legal for this call.
///
/// Two things are certified, and NEITHER is about rounding — the vector arm
/// computes the same i32 expression as [`wiener_h_row`] / [`wiener_v_row`],
/// term for term:
///
/// 1. **`w` fits the ring.** `wiener_filter_stripe` (restoration.c:399) asks
///    for `procunit_width.min((stripe_width - j + 15) & !15)`, i.e. 16, 32, 48
///    or 64 for luma and 16 or 32 for chroma — always `<= WIENER_MAX_PROC_W`.
///    Anything wider takes the scalar path.
/// 2. **The vertical pass may fold the symmetric taps.** C's
///    `wiener_convolve_v_tap7_kernel_avx512` (wiener_convolve_avx512.c:191)
///    adds `s[0]+s[6]`, `s[1]+s[5]`, `s[2]+s[4]` before multiplying, halving
///    the multiplies. That is only equal to the unfolded sum when the filter
///    is symmetric with `f[7] = 0`, which `finalize_sym_filter`
///    (restoration_pick.c:1003) and `WienerInfo::default` both guarantee — but
///    this function is public and takes an arbitrary `&[i16; 8]`, so it is
///    CHECKED, not assumed.
///
/// The `S|f| <= 4096` bounds are not a rounding constraint either; they exist
/// only so the vector arm cannot WRAP where the scalar arm would PANIC. Both
/// accumulate in i32, but magetypes' lane arithmetic wraps while the scalar
/// row's `i32` add traps on overflow in the debug profile that
/// `cargo nextest` builds. Real Wiener taps sum to `S|f| <= 286`
/// (restoration.h:141-147), so the bound is three orders of magnitude of slack
/// and never steers an encoder call to the scalar path.
fn wiener_simd_applicable(hfilter: &[i16; 8], vfilter: &[i16; 8], w: usize) -> bool {
    if w == 0 || w > WIENER_MAX_PROC_W {
        return false;
    }
    if vfilter[0] != vfilter[6]
        || vfilter[1] != vfilter[5]
        || vfilter[2] != vfilter[4]
        || vfilter[7] != 0
        || hfilter[0] != hfilter[6]
        || hfilter[1] != hfilter[5]
        || hfilter[2] != hfilter[4]
        || hfilter[7] != 0
    {
        return false;
    }
    let habs: i32 = hfilter.iter().map(|&f| i32::from(f).abs()).sum();
    let vabs: i32 = vfilter.iter().map(|&f| i32::from(f).abs()).sum();
    habs <= 4096 && vabs <= 4096
}

#[cfg(test)]
mod tests;
mod wiener_conv;
pub use wiener_conv::*;
mod stats;
pub use stats::*;
mod stripe;
pub use stripe::*;

mod hbd;
pub use hbd::*;
