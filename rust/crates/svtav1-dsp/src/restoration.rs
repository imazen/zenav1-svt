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

/// ROUND_POWER_OF_TWO on a signed value — C macro with arithmetic shift
/// semantics (gcc), identical to Rust `>>` on i32.
#[inline(always)]
fn round_power_of_two(value: i32, n: i32) -> i32 {
    (value + (1 << (n - 1))) >> n
}

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
    {
        return false;
    }
    let habs: i32 = hfilter.iter().map(|&f| i32::from(f).abs()).sum();
    let vabs: i32 = vfilter.iter().map(|&f| i32::from(f).abs()).sum();
    habs <= 4096 && vabs <= 4096
}

/// Which tier the `incant!` in [`wiener_convolve_add_src`] resolves to on the
/// CPU running the tests.
///
/// This exists because "the `_v4` arm is compiled" and "the `_v4` arm RUNS"
/// are different facts, and this repo has confused them fourteen times. The
/// tier list here is a CHARACTER-FOR-CHARACTER copy of the dispatch list in
/// `wiener_convolve_add_src`, so `incant!` expands to the same summon ladder
/// and the name this returns is the arm that ladder selects.
#[cfg(test)]
#[magetypes(v4(cfg(avx512)), v3, neon, wasm128, scalar)]
fn wiener_simd_tier_name(_token: Token) -> &'static str {
    core::any::type_name::<Token>()
}

#[cfg(test)]
fn wiener_simd_tier() -> &'static str {
    incant!(
        wiener_simd_tier_name(),
        [v4(cfg(avx512)), v3, neon, wasm128, scalar]
    )
}

/// The vector arm of [`wiener_convolve_add_src`] — one body, five tiers,
/// including x86's first genuine 512-bit arm in this port.
///
/// # Why `#[magetypes]` here, when the other kernels are per-ISA `#[arcane]`
///
/// `intra_pred`, `residual` and `me_sad` each carry a "why not `#[magetypes]`"
/// note: they need an integer WIDENING conversion, and the PUBLISHED
/// magetypes 0.9.28 (the one in `Cargo.lock`; verified with
/// `cargo read magetypes`, not from the local `~/work/archmage` checkout,
/// which carries unpublished `widen_low` / `narrow_saturating_*` work that
/// crates.io does not have) has none. That note is correct and unchanged.
///
/// This kernel sidesteps it: it never converts between lane widths. Every
/// value lives in `i32x16` from the moment the source byte is read, and the
/// u8 -> i32 widening is a plain `for t { s32[t] = i32::from(src[..]) }` loop
/// that LLVM lowers to `vpmovzxbd` inside each tier's `#[target_feature]`
/// region. The only magetypes surface used is `i32x16::{splat, from_slice,
/// store, min, max, shr_arithmetic_const}`, `Add` and `Mul<i32>` — all of them
/// in the published crate, and all of them backed by `type Repr = __m512i`
/// for `X64V4Token`, so the `_v4` arm really is 512 bits wide.
///
/// The cost of staying in i32 is the horizontal pass: C keeps it in i16 lanes
/// (`maddubs` on byte-shuffled data, 32 lanes per register) where this runs 16.
/// Closing that needs two primitives magetypes does not export —
/// `madd_adjacent` (`_mm512_madd_epi16`) and a u8 -> i16 widen — which is
/// archmage issue #89's list, already tracking `madd_adjacent` / `abs_diff`.
/// Until then the i32 form is what one body can honestly deliver on five
/// tiers, and it is the difference between a vector arm and none.
///
/// # The oracle
///
/// C's `svt_av1_wiener_convolve_add_src_avx512`
/// (`Source/Lib/ASM_AVX512/wiener_convolve_avx512.c:270`). One of its
/// rearrangements is reproduced and one is deliberately not:
///
/// * **Reproduced** — `:296` folds the `add_src` term into the coefficient
///   (`coeffs_y + offset_0`, a `1 << FILTER_BITS` planted at lane 3), so the
///   centre column is `r3 * (f[3] + 128)` rather than a separate `r3 << 7`.
///   This arm does the same on BOTH passes, saving one multiply each. It is an
///   exact regrouping of the scalar row's
///   `(s[x+3] << FILTER_BITS) + S s[x+k]*f[k]`, not an approximation.
/// * **Not reproduced** — `calc_zero_coef` (`:274`) specialises to 3-, 5- and
///   7-tap forms when the outer taps are zero, and `wiener_clip_avx512`
///   (`:27`) shifts before adding the centre sample so the whole horizontal
///   pass fits in i16. The first is a call-shape optimisation this port can
///   revisit; the second is only needed by an i16 accumulator.
#[allow(clippy::too_many_arguments)]
#[magetypes(define(i32x16), v4(cfg(avx512)), v3, neon, wasm128, scalar)]
fn wiener_convolve_simd(
    token: Token,
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_origin: usize,
    dst_stride: usize,
    hfilter: &[i16; 8],
    vfilter: &[i16; 8],
    w: usize,
    h: usize,
) {
    const LANES: usize = 16;
    let ih = h + 6;
    let n_src = w + 7;

    // Horizontal coefficients with the `add_src` term folded into the centre
    // tap: `(s[x+3] << FILTER_BITS)` IS `s[x+3] * (1 << FILTER_BITS)`.
    let mut hc = [0i32; 8];
    for k in 0..8 {
        hc[k] = i32::from(hfilter[k]);
    }
    hc[3] += 1 << FILTER_BITS;

    let zero = i32x16::splat(token, 0);
    let hi_h = i32x16::splat(
        token,
        (1i32 << (8 + 1 + FILTER_BITS - WIENER_ROUND0_BITS)) - 1,
    );
    let hi_v = i32x16::splat(token, 255);
    // `(1 << (bd + FILTER_BITS - 1))` seed plus the round-0 rounding term.
    let bias_h = i32x16::splat(
        token,
        (1 << (8 + FILTER_BITS - 1)) + (1 << (WIENER_ROUND0_BITS - 1)),
    );
    // `-(1 << (bd + round_1 - 1))` seed plus the round-1 rounding term — the
    // same constant C builds as `round_v` at wiener_convolve_avx512.c:283.
    let bias_v = i32x16::splat(
        token,
        (1 << (WIENER_ROUND1_BITS - 1)) - (1 << (8 + WIENER_ROUND1_BITS - 1)),
    );
    let vc0 = i32::from(vfilter[0]);
    let vc1 = i32::from(vfilter[1]);
    let vc2 = i32::from(vfilter[2]);
    let vc3 = i32::from(vfilter[3]) + (1 << FILTER_BITS);

    let mut ring = [[0i32; WIENER_SIMD_RING_W]; 8];
    let mut s32 = [0i32; WIENER_SIMD_SRC_W];
    let mut out32 = [0i32; WIENER_SIMD_RING_W];

    // C receives `src - 3 * stride` and subtracts three columns internally, so
    // intermediate row `j` is built from source row `j - 3` starting at
    // column -3 — the same indexing as the scalar streamed form below.
    let h_row_base = |j: usize| -> usize {
        ((src_origin + j * src_stride) as isize - 3 * src_stride as isize - 3) as usize
    };

    // One horizontal-pass row into `out`. `j >= ih` is the row C memsets: its
    // vertical weight is zero, but the sample is read.
    let fill =
        |j: usize, s32: &mut [i32; WIENER_SIMD_SRC_W], out: &mut [i32; WIENER_SIMD_RING_W]| {
            if j >= ih {
                out.fill(0);
                return;
            }
            let base = h_row_base(j);
            // Widen the row's `w + 7` source bytes once (LLVM: `vpmovzxbd`).
            // Zero-filling the tail is load-bearing — the last vector block reads
            // past `n_src`, and a zero there keeps stale data from the previous
            // row out of lanes that a narrower `w` would otherwise carry forward.
            for t in 0..n_src {
                s32[t] = i32::from(src[base + t]);
            }
            for t in n_src..WIENER_SIMD_SRC_W {
                s32[t] = 0;
            }

            let mut x = 0;
            while x < w {
                let mut acc = bias_h;
                for k in 0..8 {
                    acc = acc + i32x16::from_slice(token, &s32[x + k..]) * hc[k];
                }
                let v = acc
                    .shr_arithmetic_const::<WIENER_ROUND0_BITS>()
                    .max(zero)
                    .min(hi_h);
                let slot: &mut [i32; LANES] = (&mut out[x..x + LANES]).try_into().unwrap();
                v.store(slot);
                x += LANES;
            }
        };

    for j in 0..8 {
        fill(j, &mut s32, &mut ring[j % 8]);
    }

    for y in 0..h {
        let mut x = 0;
        while x < w {
            let r0 = i32x16::from_slice(token, &ring[y % 8][x..]);
            let r1 = i32x16::from_slice(token, &ring[(y + 1) % 8][x..]);
            let r2 = i32x16::from_slice(token, &ring[(y + 2) % 8][x..]);
            let r3 = i32x16::from_slice(token, &ring[(y + 3) % 8][x..]);
            let r4 = i32x16::from_slice(token, &ring[(y + 4) % 8][x..]);
            let r5 = i32x16::from_slice(token, &ring[(y + 5) % 8][x..]);
            let r6 = i32x16::from_slice(token, &ring[(y + 6) % 8][x..]);
            // Row 7's weight is zero (checked by `wiener_simd_applicable`), so
            // it is not loaded at all.
            let acc = bias_v + (r0 + r6) * vc0 + (r1 + r5) * vc1 + (r2 + r4) * vc2 + r3 * vc3;
            let v = acc
                .shr_arithmetic_const::<WIENER_ROUND1_BITS>()
                .max(zero)
                .min(hi_v);
            let slot: &mut [i32; LANES] = (&mut out32[x..x + LANES]).try_into().unwrap();
            v.store(slot);
            x += LANES;
        }
        let d = dst_origin + y * dst_stride;
        for i in 0..w {
            dst[d + i] = out32[i] as u8;
        }
        if y + 8 <= ih {
            fill(y + 8, &mut s32, &mut ring[(y + 8) % 8]);
        }
    }
}

/// C `svt_av1_wiener_convolve_add_src_c` (convolve.c:106), 8-bit.
///
/// `src`/`dst` are whole padded planes; `src_origin`/`dst_origin` index the
/// top-left pixel of the `w x h` block. Margins REQUIRED in-bounds around the
/// block in `src`: 3 above, 3 left, 3 below, 4 right (the 8th tap is zero but
/// the C code reads the sample; this port reads it too so the fuzz proves the
/// exact access pattern is safe on our padded planes).
///
/// `hfilter`/`vfilter` are full 8-tap rows (tap\[7\] = 0 by construction).
/// round0/round1 are `get_conv_params_wiener(8)`: 3 and 11.
///
/// # Streamed, and row-major on BOTH passes
///
/// C materialises the whole `(h + 7) x w` `uint16_t` intermediate, and so did
/// this port — one heap allocation per processing unit, on a function the
/// loop-restoration filter calls once per 64-wide column of every stripe of
/// every frame. Output row `y` reads intermediate rows `y .. y + 7`, so the
/// dependency is seven rows deep and the whole thing streams through a ring of
/// eight rows on the stack.
///
/// The vertical pass ALSO ran `for x { for y { .. } }`, i.e. column-major over
/// both the intermediate and the destination — every inner step jumped a
/// stride. C's `svt_aom_convolve_add_src_vert_hip` is row-major. Reordering two
/// independent loops changes nothing arithmetically and is byte-identical by
/// construction; it is called out here because it is the kind of change that
/// looks like a rewrite in a diff and is not one.
///
/// Intermediate row `h + 6` (the last one output row `h - 1` reads) is past
/// what the horizontal pass produces; C memsets it and its taps are weight 0.
/// The ring zero-fills it for the same reason.
#[allow(clippy::too_many_arguments)]
pub fn wiener_convolve_add_src(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_origin: usize,
    dst_stride: usize,
    hfilter: &[i16; 8],
    vfilter: &[i16; 8],
    w: usize,
    h: usize,
) {
    if w > WIENER_MAX_PROC_W {
        return wiener_convolve_add_src_materialised(
            src, src_origin, src_stride, dst, dst_origin, dst_stride, hfilter, vfilter, w, h,
        );
    }

    if wiener_simd_applicable(hfilter, vfilter, w) {
        return incant!(
            wiener_convolve_simd(
                src, src_origin, src_stride, dst, dst_origin, dst_stride, hfilter, vfilter, w, h,
            ),
            [v4(cfg(avx512)), v3, neon, wasm128, scalar]
        );
    }

    let bd = 8i32;
    let ih = h + 6;
    let clamp_limit = (1i32 << (bd + 1 + FILTER_BITS - WIENER_ROUND0_BITS)) - 1;

    // Ring of the eight intermediate rows output row `y` needs. Row `j` of the
    // intermediate lives at `ring[j % 8]`.
    let mut ring = [[0u16; WIENER_MAX_PROC_W]; 8];

    // C receives `src - 3 * stride` and subtracts three columns internally, so
    // intermediate row `j` is built from source row `j - 3` starting at
    // column -3.
    let h_row_base = |j: usize| -> usize {
        ((src_origin + j * src_stride) as isize - 3 * src_stride as isize - 3) as usize
    };

    let fill = |ring: &mut [[u16; WIENER_MAX_PROC_W]; 8], j: usize| {
        if j < ih {
            let base = h_row_base(j);
            wiener_h_row(&src[base..], w, hfilter, clamp_limit, &mut ring[j % 8]);
        } else {
            // Intermediate row `ih` is the one C memsets; weight 0, but read.
            ring[j % 8][..w].fill(0);
        }
    };

    for j in 0..8 {
        fill(&mut ring, j);
    }

    let mut out_row = [0u8; WIENER_MAX_PROC_W];
    for y in 0..h {
        {
            let rows: [&[u16]; 8] = [
                &ring[y % 8],
                &ring[(y + 1) % 8],
                &ring[(y + 2) % 8],
                &ring[(y + 3) % 8],
                &ring[(y + 4) % 8],
                &ring[(y + 5) % 8],
                &ring[(y + 6) % 8],
                &ring[(y + 7) % 8],
            ];
            wiener_v_row(&rows, w, vfilter, &mut out_row);
        }
        let d = dst_origin + y * dst_stride;
        dst[d..d + w].copy_from_slice(&out_row[..w]);
        if y + 8 <= ih {
            fill(&mut ring, y + 8);
        }
    }
}

/// The materialised form C writes: the whole `(h + 7) x w` intermediate, both
/// passes over it. Kept as the `w > WIENER_MAX_PROC_W` fallback AND as the
/// oracle [`wiener_convolve_add_src`]'s streamed form is pinned against
/// (`wiener_streaming_matches_materialised`).
#[allow(clippy::too_many_arguments)]
pub fn wiener_convolve_add_src_materialised(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_origin: usize,
    dst_stride: usize,
    hfilter: &[i16; 8],
    vfilter: &[i16; 8],
    w: usize,
    h: usize,
) {
    let bd = 8i32;
    // intermediate_height = (((h - 1) * 16 + 0) >> 4) + 8 - 1 = h + 6
    let ih = h + 6;
    // Temp rows ih + 1: the C memsets one row past the end (the 8th tap of
    // the bottom-most vertical windows reads it, times weight 0). Zero-init
    // covers it.
    let mut temp = alloc::vec![0u16; (ih + 1) * w.max(1)];
    let tstride = w;

    // --- Horizontal pass (svt_aom_convolve_add_src_horiz_hip) ---
    // C receives src - 3*stride, then subtracts 3 columns internally; rows
    // -3..h+2 relative to the block, window cols x-3..x+4.
    let clamp_limit = (1i32 << (bd + 1 + FILTER_BITS - WIENER_ROUND0_BITS)) - 1;
    for y in 0..ih {
        // Block-relative source row (y - 3), as index into the plane.
        let row_base = (src_origin + y * src_stride) as isize - 3 * src_stride as isize;
        for x in 0..w {
            let px = |k: usize| -> i32 {
                let idx = row_base + x as isize + k as isize - 3;
                src[idx as usize] as i32
            };
            let mut sum: i32 = (px(3) << FILTER_BITS) + (1 << (bd + FILTER_BITS - 1));
            for (k, &f) in hfilter.iter().enumerate() {
                sum += px(k) * f as i32;
            }
            temp[y * tstride + x] =
                round_power_of_two(sum, WIENER_ROUND0_BITS).clamp(0, clamp_limit) as u16;
        }
    }

    // --- Vertical pass (svt_aom_convolve_add_src_vert_hip) ---
    // C receives temp + 3*stride then subtracts 3 rows; window rows y..y+7
    // in temp coordinates (top-most window centered on block row 0).
    for x in 0..w {
        for y in 0..h {
            let base = y * tstride + x;
            let center = temp[base + 3 * tstride] as i32;
            let mut sum: i32 = (center << FILTER_BITS) - (1 << (bd + WIENER_ROUND1_BITS - 1));
            for (k, &f) in vfilter.iter().enumerate() {
                sum += temp[base + k * tstride] as i32 * f as i32;
            }
            dst[dst_origin + y * dst_stride + x] =
                round_power_of_two(sum, WIENER_ROUND1_BITS).clamp(0, 255) as u8;
        }
    }
}

/// C `find_average` (restoration_pick.h:21).
pub fn find_average(
    src: &[u8],
    origin: usize,
    stride: usize,
    h_start: i32,
    h_end: i32,
    v_start: i32,
    v_end: i32,
) -> u8 {
    let mut sum: u64 = 0;
    for i in v_start..v_end {
        for j in h_start..h_end {
            let idx = origin as isize + i as isize * stride as isize + j as isize;
            sum += src[idx as usize] as u64;
        }
    }
    (sum / ((v_end - v_start) as u64 * (h_end - h_start) as u64)) as u8
}

/// C `svt_av1_compute_stats_c` (restoration_pick.c:652).
///
/// `m` must hold `win*win` entries, `h` `win^2 * win^2`. `dgd` needs
/// `win/2` margins around the region (the search extends the recon by 3+
/// before calling). Coordinates are plane-relative; `origin` indexes (0,0).
///
/// Runtime-dispatched (`incant!([v3, neon, scalar])`): the AVX2 (`_v3`) and
/// NEON arms are C's six-step kernel (`compute_stats_win{5,7}_avx2` /
/// `_neon`) — full madd dots for `M` and the first block row/column of `H`,
/// every other `H` entry derived from a neighbour by an exact O(width) or
/// O(height) shift delta — written once in `cs_kernel!` over seven per-ISA
/// lane primitives; the `_scalar` arm is the verbatim C transcription. All
/// three are byte-identical (every intermediate is an exact integer; see the
/// macro's doc). Pinned by `tests/c_parity_wiener.rs` (`compute_stats_matches_c`
/// on the host tier + `compute_stats_all_tiers_match_c` forcing every tier).
#[allow(clippy::too_many_arguments)]
pub fn compute_stats(
    wiener_win: usize,
    dgd: &[u8],
    dgd_origin: usize,
    dgd_stride: usize,
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    h_start: i32,
    h_end: i32,
    v_start: i32,
    v_end: i32,
    m: &mut [i64],
    h: &mut [i64],
) {
    incant!(
        compute_stats_impl(
            wiener_win, dgd, dgd_origin, dgd_stride, src, src_origin, src_stride, h_start, h_end,
            v_start, v_end, m, h
        ),
        [v3, neon, scalar]
    )
}

#[allow(clippy::too_many_arguments)]
fn compute_stats_impl_scalar(
    _token: ScalarToken,
    wiener_win: usize,
    dgd: &[u8],
    dgd_origin: usize,
    dgd_stride: usize,
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    h_start: i32,
    h_end: i32,
    v_start: i32,
    v_end: i32,
    m: &mut [i64],
    h: &mut [i64],
) {
    compute_stats_scalar_core(
        wiener_win, dgd, dgd_origin, dgd_stride, src, src_origin, src_stride, h_start, h_end,
        v_start, v_end, m, h,
    );
}

// ---- AVX2 lane primitives for `cs_kernel!` ------------------------------
//
// One `__m256i` holds 16 `i16` (a window-row chunk) or 8 `i32` (an
// accumulator). `cs_madd_v3` is C's `madd_avx2` (`pickrst_avx2.h:235`):
// `_mm256_madd_epi16` forms the 16 products, sums ADJACENT pairs into 8
// `i32` lanes, and the add accumulates — one instruction pair per 16 MACs.

#[cfg(target_arch = "x86_64")]
#[rite]
fn cs_load_v3(_t: Desktop64, a: &[i16; 16]) -> __m256i {
    _mm256_loadu_si256(a)
}

/// Lane mask: lanes `< n` all-ones, the rest zero (`n <= 16`).
#[cfg(target_arch = "x86_64")]
#[rite]
fn cs_mask_v3(_t: Desktop64, n: usize) -> __m256i {
    let mut a = [0i16; 16];
    for v in a.iter_mut().take(n) {
        *v = -1;
    }
    _mm256_loadu_si256(&a)
}

#[cfg(target_arch = "x86_64")]
#[rite]
fn cs_and_v3(_t: Desktop64, a: __m256i, b: __m256i) -> __m256i {
    _mm256_and_si256(a, b)
}

#[cfg(target_arch = "x86_64")]
#[rite]
fn cs_zero_v3(_t: Desktop64) -> __m256i {
    _mm256_setzero_si256()
}

#[cfg(target_arch = "x86_64")]
#[rite]
fn cs_madd_v3(_t: Desktop64, acc: __m256i, a: __m256i, b: __m256i) -> __m256i {
    _mm256_add_epi32(acc, _mm256_madd_epi16(a, b))
}

#[cfg(target_arch = "x86_64")]
#[rite]
fn cs_msub_v3(_t: Desktop64, acc: __m256i, a: __m256i, b: __m256i) -> __m256i {
    _mm256_sub_epi32(acc, _mm256_madd_epi16(a, b))
}

/// Horizontal sum of the eight `i32` lanes, widened BEFORE adding (the
/// lane total may exceed `i32` even though each lane cannot).
#[cfg(target_arch = "x86_64")]
#[rite]
fn cs_reduce_v3(_t: Desktop64, acc: __m256i) -> i64 {
    let mut a = [0i32; 8];
    _mm256_storeu_si256(&mut a, acc);
    a.iter().map(|&v| i64::from(v)).sum()
}

/// C's `find_average_avx2` (`pickrst_avx2.c:24`): the region's mean pixel,
/// summed 32 bytes at a time with `_mm256_sad_epu8` against zero into four
/// `u64` lanes. Exact — the same `u64` sum and truncating divide as
/// [`find_average`], so the same `u8`.
#[cfg(target_arch = "x86_64")]
#[rite]
#[allow(clippy::too_many_arguments)]
fn cs_find_average_v3(
    _t: Desktop64,
    src: &[u8],
    origin: usize,
    stride: usize,
    h_start: i32,
    h_end: i32,
    v_start: i32,
    v_end: i32,
) -> u8 {
    let width = (h_end - h_start) as usize;
    let height = (v_end - v_start) as usize;
    let zero = _mm256_setzero_si256();
    let mut acc = _mm256_setzero_si256();
    let mut tail: u64 = 0;
    for r in 0..height {
        let base = (origin as isize
            + (v_start as isize + r as isize) * stride as isize
            + h_start as isize) as usize;
        let row = &src[base..base + width];
        let (c32, rem) = row.as_chunks::<32>();
        for ch in c32 {
            acc = _mm256_add_epi64(acc, _mm256_sad_epu8(_mm256_loadu_si256(ch), zero));
        }
        for &p in rem {
            tail += u64::from(p);
        }
    }
    let mut lanes = [0u64; 4];
    _mm256_storeu_si256(&mut lanes, acc);
    let sum = lanes.iter().sum::<u64>() + tail;
    (sum / (width as u64 * height as u64)) as u8
}

// ---- NEON lane primitives for `cs_kernel!` ------------------------------
//
// A 16-lane `i16` chunk is two `int16x8_t`; an accumulator is two
// `int32x4_t`. `cs_madd_neon` is C's `madd_neon` (`pickrst_neon.h:46`):
// `vmlal_s16` / `vmlal_high_s16` widen-multiply-accumulate 4 lanes each, so
// one accumulator lane receives TWO products per chunk, the same per-lane
// growth as the AVX2 pairwise `madd` — the drain interval below is
// ISA-independent.

#[cfg(target_arch = "aarch64")]
#[derive(Clone, Copy)]
struct CsV(int16x8_t, int16x8_t);

#[cfg(target_arch = "aarch64")]
#[derive(Clone, Copy)]
struct CsA(int32x4_t, int32x4_t);

#[cfg(target_arch = "aarch64")]
#[rite]
fn cs_load_neon(_t: NeonToken, a: &[i16; 16]) -> CsV {
    let lo: &[i16; 8] = a[..8].try_into().unwrap();
    let hi: &[i16; 8] = a[8..].try_into().unwrap();
    CsV(vld1q_s16(lo), vld1q_s16(hi))
}

/// Lane mask: lanes `< n` all-ones, the rest zero (`n <= 16`).
#[cfg(target_arch = "aarch64")]
#[rite]
fn cs_mask_neon(_t: NeonToken, n: usize) -> CsV {
    let mut a = [0i16; 16];
    for v in a.iter_mut().take(n) {
        *v = -1;
    }
    let lo: &[i16; 8] = a[..8].try_into().unwrap();
    let hi: &[i16; 8] = a[8..].try_into().unwrap();
    CsV(vld1q_s16(lo), vld1q_s16(hi))
}

#[cfg(target_arch = "aarch64")]
#[rite]
fn cs_and_neon(_t: NeonToken, a: CsV, b: CsV) -> CsV {
    CsV(vandq_s16(a.0, b.0), vandq_s16(a.1, b.1))
}

#[cfg(target_arch = "aarch64")]
#[rite]
fn cs_zero_neon(_t: NeonToken) -> CsA {
    CsA(vdupq_n_s32(0), vdupq_n_s32(0))
}

#[cfg(target_arch = "aarch64")]
#[rite]
fn cs_madd_neon(_t: NeonToken, acc: CsA, a: CsV, b: CsV) -> CsA {
    let a0 = vmlal_s16(acc.0, vget_low_s16(a.0), vget_low_s16(b.0));
    let a0 = vmlal_high_s16(a0, a.0, b.0);
    let a1 = vmlal_s16(acc.1, vget_low_s16(a.1), vget_low_s16(b.1));
    let a1 = vmlal_high_s16(a1, a.1, b.1);
    CsA(a0, a1)
}

#[cfg(target_arch = "aarch64")]
#[rite]
fn cs_msub_neon(_t: NeonToken, acc: CsA, a: CsV, b: CsV) -> CsA {
    let a0 = vmlsl_s16(acc.0, vget_low_s16(a.0), vget_low_s16(b.0));
    let a0 = vmlsl_high_s16(a0, a.0, b.0);
    let a1 = vmlsl_s16(acc.1, vget_low_s16(a.1), vget_low_s16(b.1));
    let a1 = vmlsl_high_s16(a1, a.1, b.1);
    CsA(a0, a1)
}

/// Horizontal sum of the eight `i32` lanes, widening as it adds.
#[cfg(target_arch = "aarch64")]
#[rite]
fn cs_reduce_neon(_t: NeonToken, acc: CsA) -> i64 {
    vaddlvq_s32(acc.0) + vaddlvq_s32(acc.1)
}

/// C's `find_average_neon`: the region's mean pixel, 16 bytes at a time
/// through the pairwise-widening adds (`u8 -> u16 -> u32 -> u64`). Exact —
/// the same `u64` sum and truncating divide as [`find_average`].
#[cfg(target_arch = "aarch64")]
#[rite]
#[allow(clippy::too_many_arguments)]
fn cs_find_average_neon(
    _t: NeonToken,
    src: &[u8],
    origin: usize,
    stride: usize,
    h_start: i32,
    h_end: i32,
    v_start: i32,
    v_end: i32,
) -> u8 {
    let width = (h_end - h_start) as usize;
    let height = (v_end - v_start) as usize;
    let mut acc = vdupq_n_u64(0);
    let mut tail: u64 = 0;
    for r in 0..height {
        let base = (origin as isize
            + (v_start as isize + r as isize) * stride as isize
            + h_start as isize) as usize;
        let row = &src[base..base + width];
        let (c16, rem) = row.as_chunks::<16>();
        for ch in c16 {
            acc = vpadalq_u32(acc, vpaddlq_u16(vpaddlq_u8(vld1q_u8(ch))));
        }
        for &p in rem {
            tail += u64::from(p);
        }
    }
    let sum = vaddvq_u64(acc) + tail;
    (sum / (width as u64 * height as u64)) as u8
}

// ---- the shared C-shape kernel ------------------------------------------

/// The largest region dimension (width OR height) the C-shape SIMD arms
/// accept; a larger region takes the scalar reference. Every accumulator
/// bound below is derived from this number, so it is STRUCTURAL: the
/// row-delta and column-delta accumulators hold `2 * ceil(dim / 16)` pairwise
/// products per lane with no drain, i.e. at most
/// `2 * 2000 * 130_050 = 5.2e8 < i32::MAX`, and the step-1/2 accumulators
/// drain to `i64` every `rows_per_drain` rows (computed from the width, and
/// `>= 8` at this bound). Real callers are bounded by
/// `RESTORATION_UNITSIZE_MAX * 3 / 2 = 384` on both axes.
const CS_MAX_DIM: usize = 32_000;

/// Which regions the C-shape arms take (the rest go to the scalar core):
/// C's two window sizes only (`compute_stats_win{5,7}`), non-empty, and
/// inside [`CS_MAX_DIM`] on both axes.
fn cs_accepts(wiener_win: usize, width: usize, height: usize) -> bool {
    (wiener_win == WIENER_WIN || wiener_win == WIENER_WIN_CHROMA)
        && width != 0
        && height != 0
        && width <= CS_MAX_DIM
        && height <= CS_MAX_DIM
}

/// Geometry of the three sub-average planes `cs_kernel!` reads (all `i16`,
/// all with zeroed padding):
///
/// * `d` — the window support, `dh = height + 2*hw` rows of `dw = width +
///   2*hw` values at row stride `ds = ceil16(width) + 2*hw`. A 16-lane load
///   at column `x + off` with `x < ceil16(width)` and `off <= 2*hw` stays
///   inside the row; the padding `[dw, ds)` is zero.
/// * `s` — the source region, `height` rows of `width` at stride
///   `ss = ceil16(width)`, zero beyond `width` (so `s` needs no lane mask).
/// * `t` — TRANSPOSED strips of `d`: the `2*hw` leftmost columns
///   (`0 .. 2*hw`) and the `2*hw` columns at `width .. width + 2*hw`, each
///   laid out contiguously along the rows (`dh` values at stride
///   `ts = ceil16(height) + 2*hw`). These are the only columns the
///   column-shift deltas (steps 3-4) touch; transposing them once turns
///   C's scalar `_mm256_insert_epi16` column gathers into the same masked
///   madd dot the rest of the kernel uses.
struct CsGeom {
    width: usize,
    height: usize,
    hw: usize,
    dw: usize,
    dh: usize,
    ds: usize,
    ss: usize,
    ts: usize,
}

impl CsGeom {
    fn new(wiener_win: usize, width: usize, height: usize) -> Self {
        let hw = wiener_win >> 1;
        let up16 = |v: usize| (v + 15) & !15;
        CsGeom {
            width,
            height,
            hw,
            dw: width + 2 * hw,
            dh: height + 2 * hw,
            ds: up16(width) + 2 * hw,
            ss: up16(width),
            ts: up16(height) + 2 * hw,
        }
    }
    fn d_len(&self) -> usize {
        self.dh * self.ds
    }
    fn s_len(&self) -> usize {
        self.height * self.ss
    }
    fn t_len(&self) -> usize {
        4 * self.hw * self.ts
    }
}

/// C's `sub_avg_block_avx2` (`pickrst_avx2.c:202`) / NEON `compute_sub_avg`,
/// plus the transposed edge strips. Plain scalar code — LLVM vectorises the
/// subtract; it is one pass over the region against the kernel's ~134
/// multiply-accumulates per pixel.
#[allow(clippy::too_many_arguments)]
fn cs_prepare(
    g: &CsGeom,
    avg: i16,
    dgd: &[u8],
    dgd_origin: usize,
    dgd_stride: usize,
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    h_start: i32,
    v_start: i32,
    d: &mut [i16],
    s: &mut [i16],
    t: &mut [i16],
) {
    let hw = g.hw;
    let d_row0 = dgd_origin as isize
        + (v_start as isize - hw as isize) * dgd_stride as isize
        + (h_start as isize - hw as isize);
    for r in 0..g.dh {
        let base = (d_row0 + r as isize * dgd_stride as isize) as usize;
        let srcrow = &dgd[base..base + g.dw];
        let drow = &mut d[r * g.ds..(r + 1) * g.ds];
        for (o, &p) in drow[..g.dw].iter_mut().zip(srcrow) {
            *o = p as i16 - avg;
        }
        drow[g.dw..].fill(0);
    }
    let s_row0 = src_origin as isize + v_start as isize * src_stride as isize + h_start as isize;
    for r in 0..g.height {
        let base = (s_row0 + r as isize * src_stride as isize) as usize;
        let srcrow = &src[base..base + g.width];
        let srow = &mut s[r * g.ss..(r + 1) * g.ss];
        for (o, &p) in srow[..g.width].iter_mut().zip(srcrow) {
            *o = p as i16 - avg;
        }
        srow[g.width..].fill(0);
    }
    // Strips: k < 2*hw is column k; k >= 2*hw is column width + (k - 2*hw).
    for k in 0..4 * hw {
        let col = if k < 2 * hw {
            k
        } else {
            g.width + (k - 2 * hw)
        };
        let trow = &mut t[k * g.ts..(k + 1) * g.ts];
        for (r, o) in trow[..g.dh].iter_mut().enumerate() {
            *o = d[r * g.ds + col];
        }
        trow[g.dh..].fill(0);
    }
}

/// Mirror the upper triangle of `H` into the lower — C's
/// `diagonal_copy_stats_avx2` (`pickrst_avx2.c:685`).
fn cs_mirror(win2: usize, h: &mut [i64]) {
    for k in 0..win2 {
        for l in (k + 1)..win2 {
            h[l * win2 + k] = h[k * win2 + l];
        }
    }
}

/// The M/H accumulation in the shape of C's `compute_stats_win{5,7}_avx2`
/// (`pickrst_avx2.c:775` / `:1546`; the NEON twins at `pickrst_neon.c:147` /
/// `:698` are the same six steps). Written once; the ISA supplies seven lane
/// primitives (a 16-lane `i16` load, a lane AND, a zero accumulator, a
/// pairwise multiply-ADD and multiply-SUBTRACT into `i32` lanes, an
/// `i64` horizontal reduce, and a lane mask). `$W` is a literal so the
/// per-window accumulator arrays are register-resident.
///
/// **Indexing.** `H[k][t]` with `k = kk * W + l`, `t = tt * W + m`: `kk`, `tt`
/// are window COLUMN offsets and `l`, `m` window ROW offsets (C's `idx`
/// order). On the sub-average planes, with `d[r][c]` the window support,
///
/// ```text
///   H[(kk,l)][(tt,m)] = sum_{r<height} sum_{c<width} d[r+l][c+kk] * d[r+m][c+tt]
///   M[(kk,l)]         = sum_{r,c}                     s[r][c]      * d[r+l][c+kk]
/// ```
///
/// View H as a `W x W` grid of `W x W` blocks, block `(kk, tt)`. Only blocks
/// with `tt >= kk` are computed (and inside a diagonal block only `m >= l`);
/// [`cs_mirror`] fills the rest.
///
/// **The six steps** (C's numbering):
/// 1. Every `M` entry and block `(0, tt)`'s TOP ROW for every `tt` — full
///    dots over the region. C's `stats_top_win*` (`pickrst_avx2.c:271`).
/// 2. Block `(0, tt)`'s LEFT COLUMN for `tt >= 1` — full dots. C's
///    `stats_left_win*` (`:315`).
/// 3-4. Every other block's top row and left column, from block
///      `(kk-1, tt-1)`'s by a COLUMN-shift delta over the height:
///      `sum_r d[r+l][width+kk-1] * d[r+m][width+tt-1] - d[r+l][kk-1] * d[r+m][tt-1]`.
///      C's step 3 is the diagonal blocks (`:967`), step 4 the squares
///      (`:1185`); here both are one loop.
/// 5-6. Every block's interior, entry `(l, m)` from `(l-1, m-1)` by a
///      ROW-shift delta over the width:
///      `sum_c d[height+l-1][c+kk] * d[height+m-1][c+tt] - d[l-1][c+kk] * d[m-1][c+tt]`.
///      C's `derive_square_win*` (`:458`) / `derive_triangle_win*` (`:562`).
///
/// Both recurrences are exact identities (shift the summation index by one
/// and the boundary terms are the delta), so the MAC count is
/// `(2 W^2 + (W-1)^2) * width * height` for steps 1-2 plus `O((W^4) *
/// (width + height))` for the deltas — 134 per pixel at `W = 7`, against the
/// `W^2 (W^2 + 3) / 2 = 1274` of the per-pixel form this replaced.
///
/// **BYTE-IDENTITY.** Every quantity here is an exact integer: products of
/// two values in `[-255, 255]` fit `i32`, the pairwise `madd` lanes are
/// bounded by construction (see [`CS_MAX_DIM`] and `rows_per_drain` below)
/// and are widened to `i64` before any cross-lane add, and the recurrences
/// add exact `i64`s. So each `M`/`H` entry equals the same finite sum of the
/// same products the scalar reference forms in `(i, j, k, l)` order — and
/// for exact integers a sum does not depend on its association. Pinned
/// against real C on every tier by
/// `tests/c_parity_wiener.rs::compute_stats_all_tiers_match_c`.
///
/// **Tail masking.** The region width is not a multiple of 16: the last
/// chunk's lanes `>= width` must contribute zero. `s` is zero-padded, so a
/// dot with `s` needs nothing; every other dot ANDs ONE operand with a lane
/// mask (the other operand's lanes there are whatever the padding holds,
/// times zero). Same along the height for the transposed strips.
macro_rules! cs_kernel {
    ($tok:expr, $W:literal, $g:expr, $d:expr, $s:expr, $t:expr, $m:expr, $h:expr;
     $load:ident, $and:ident, $zero:ident, $madd:ident, $msub:ident, $reduce:ident, $mask:ident) => {{
        const W: usize = $W;
        const W2: usize = W * W;
        let tok = $tok;
        let g: &CsGeom = $g;
        let d: &[i16] = $d;
        let s: &[i16] = $s;
        let t: &[i16] = $t;
        let m: &mut [i64] = $m;
        let h: &mut [i64] = $h;
        let (width, height, ds, ss, ts) = (g.width, g.height, g.ds, g.ss, g.ts);
        // Chunk counts along the width and the height, and the lane masks
        // for the LAST chunk of each (all-ones when the dimension is a
        // multiple of 16). Every loop below walks whole chunks; the mask on
        // the last one zeroes the lanes past the region.
        let nw = width.div_ceil(16);
        let wmask = $mask(tok, width - (nw - 1) * 16);
        let nh = height.div_ceil(16);
        let hmask = $mask(tok, height - (nh - 1) * 16);
        let full = $mask(tok, 16);
        // Each accumulator lane grows by <= 2 * 65_025 per chunk; drain to
        // i64 before `rows_per_drain * nw` chunks could overflow it.
        let rows_per_drain = ((i32::MAX as usize) / (nw * 130_050)).max(1);

        // Row views are exact-length `[[i16; 16]]` slices (`cs_chunks`), built
        // with plain loops (NOT `core::array::from_fn`, which is not inlined
        // and hides the lengths), so `[c]` under `for c in 0..n` carries no
        // bounds check.

        // ---- Step 1: M, and block (0, j)'s top row, for every j.
        for j in 0..W {
            let mut tm = [0i64; W];
            let mut th = [0i64; W];
            let mut r0 = 0usize;
            while r0 < height {
                let r1 = (r0 + rows_per_drain).min(height);
                let mut am = [$zero(tok); W];
                let mut ah = [$zero(tok); W];
                for r in r0..r1 {
                    let srow = cs_chunks(&s[r * ss..], nw);
                    let drow = cs_chunks(&d[r * ds..], nw);
                    let mut dl = [srow; W];
                    for l in 0..W {
                        dl[l] = cs_chunks(&d[(r + l) * ds + j..], nw);
                    }
                    for c in 0..nw {
                        let mk = if c + 1 == nw { wmask } else { full };
                        let sv = $load(tok, &srow[c]);
                        let dv = $and(tok, $load(tok, &drow[c]), mk);
                        for l in 0..W {
                            let v = $load(tok, &dl[l][c]);
                            am[l] = $madd(tok, am[l], sv, v);
                            ah[l] = $madd(tok, ah[l], dv, v);
                        }
                    }
                }
                for l in 0..W {
                    tm[l] += $reduce(tok, am[l]);
                    th[l] += $reduce(tok, ah[l]);
                }
                r0 = r1;
            }
            for l in 0..W {
                m[j * W + l] = tm[l];
                h[j * W + l] = th[l];
            }
        }

        // ---- Step 2: block (0, j)'s left column, j >= 1.
        for j in 1..W {
            let mut th = [0i64; W];
            let mut r0 = 0usize;
            while r0 < height {
                let r1 = (r0 + rows_per_drain).min(height);
                let mut ah = [$zero(tok); W];
                for r in r0..r1 {
                    let drow = cs_chunks(&d[r * ds + j..], nw);
                    let mut dl = [drow; W];
                    for l in 1..W {
                        dl[l] = cs_chunks(&d[(r + l) * ds..], nw);
                    }
                    for c in 0..nw {
                        let mk = if c + 1 == nw { wmask } else { full };
                        let dj = $and(tok, $load(tok, &drow[c]), mk);
                        for l in 1..W {
                            ah[l] = $madd(tok, ah[l], dj, $load(tok, &dl[l][c]));
                        }
                    }
                }
                for l in 1..W {
                    th[l] += $reduce(tok, ah[l]);
                }
                r0 = r1;
            }
            for l in 1..W {
                h[l * W2 + j * W] = th[l];
            }
        }

        // ---- Steps 3-4: block (i, j)'s top row (and, off the diagonal, its
        // left column) from block (i-1, j-1)'s, by column-shift deltas along
        // the transposed strips. Strip k < 2*hw is column k of `d`; strip
        // 2*hw + k is column width + k. Top row and left column are two
        // passes so each keeps its W accumulators in registers.
        for i in 1..W {
            let li = (i - 1) * ts;
            let ri = (W - 1 + i - 1) * ts;
            for j in i..W {
                let lj = (j - 1) * ts;
                let rj = (W - 1 + j - 1) * ts;
                // Offset-`o` views of the four strips, o = 0..W.
                let ri0 = cs_chunks(&t[ri..], nh);
                let li0 = cs_chunks(&t[li..], nh);
                let rj0 = cs_chunks(&t[rj..], nh);
                let lj0 = cs_chunks(&t[lj..], nh);
                let mut rjo = [rj0; W];
                let mut ljo = [lj0; W];
                for o in 1..W {
                    rjo[o] = cs_chunks(&t[rj + o..], nh);
                    ljo[o] = cs_chunks(&t[lj + o..], nh);
                }
                let mut at = [$zero(tok); W];
                for c in 0..nh {
                    let mk = if c + 1 == nh { hmask } else { full };
                    let a = $and(tok, $load(tok, &ri0[c]), mk);
                    let b = $and(tok, $load(tok, &li0[c]), mk);
                    for mm in 0..W {
                        at[mm] = $madd(tok, at[mm], a, $load(tok, &rjo[mm][c]));
                        at[mm] = $msub(tok, at[mm], b, $load(tok, &ljo[mm][c]));
                    }
                }
                for mm in 0..W {
                    h[(i * W) * W2 + j * W + mm] =
                        h[((i - 1) * W) * W2 + (j - 1) * W + mm] + $reduce(tok, at[mm]);
                }
                if j > i {
                    let mut rio = [ri0; W];
                    let mut lio = [li0; W];
                    for o in 1..W {
                        rio[o] = cs_chunks(&t[ri + o..], nh);
                        lio[o] = cs_chunks(&t[li + o..], nh);
                    }
                    let mut al = [$zero(tok); W];
                    for c in 0..nh {
                        let mk = if c + 1 == nh { hmask } else { full };
                        let a = $and(tok, $load(tok, &rj0[c]), mk);
                        let b = $and(tok, $load(tok, &lj0[c]), mk);
                        for l in 1..W {
                            al[l] = $madd(tok, al[l], $load(tok, &rio[l][c]), a);
                            al[l] = $msub(tok, al[l], $load(tok, &lio[l][c]), b);
                        }
                    }
                    for l in 1..W {
                        h[(i * W + l) * W2 + j * W] =
                            h[((i - 1) * W + l) * W2 + (j - 1) * W] + $reduce(tok, al[l]);
                    }
                }
            }
        }

        // ---- Steps 5-6: every block's interior, entry (l, m) from
        // (l-1, m-1) by a row-shift delta along the width. Diagonal blocks
        // fill m >= l only.
        for i in 0..W {
            for j in i..W {
                // Rows l' = 0..W-1 (top) and height + l' (bottom) of `d`, at
                // column offsets i and j.
                let t0 = cs_chunks(&d[i..], nw);
                let mut topi = [t0; W - 1];
                let mut boti = [t0; W - 1];
                let mut topj = [t0; W - 1];
                let mut botj = [t0; W - 1];
                for o in 0..W - 1 {
                    topi[o] = cs_chunks(&d[o * ds + i..], nw);
                    boti[o] = cs_chunks(&d[(height + o) * ds + i..], nw);
                    topj[o] = cs_chunks(&d[o * ds + j..], nw);
                    botj[o] = cs_chunks(&d[(height + o) * ds + j..], nw);
                }
                for lp in 0..W - 1 {
                    let m_lo = if j == i { lp } else { 0 };
                    let mut acc = [$zero(tok); W];
                    for c in 0..nw {
                        let mk = if c + 1 == nw { wmask } else { full };
                        let ab = $and(tok, $load(tok, &boti[lp][c]), mk);
                        let atop = $and(tok, $load(tok, &topi[lp][c]), mk);
                        for mp in m_lo..W - 1 {
                            acc[mp] = $madd(tok, acc[mp], ab, $load(tok, &botj[mp][c]));
                            acc[mp] = $msub(tok, acc[mp], atop, $load(tok, &topj[mp][c]));
                        }
                    }
                    for mp in m_lo..W - 1 {
                        h[(i * W + lp + 1) * W2 + j * W + mp + 1] =
                            h[(i * W + lp) * W2 + j * W + mp] + $reduce(tok, acc[mp]);
                    }
                }
            }
        }
    }};
}

/// The first `n` 16-lane chunks of `p`, as an exact-length slice so the
/// kernel's `[c]` indexing under `for c in 0..n` carries no bounds check.
#[inline(always)]
fn cs_chunks(p: &[i16], n: usize) -> &[[i16; 16]] {
    &p[..n * 16].as_chunks::<16>().0[..n]
}

/// NEON `compute_stats` — C's `svt_av1_compute_stats_neon` shape
/// (`ASM_NEON/pickrst_neon.c:1200`), the same six-step [`cs_kernel!`] body
/// the AVX2 arm runs, on NEON lane primitives. This replaced the row-pair
/// correlation arm of 2026-09-03 (85 + 49 dot calls per row, each with its
/// own zeroed accumulators and cross-lane reduce): the C shape does the same
/// ~134 multiply-accumulates per pixel but keeps every accumulator in a
/// register across the whole region and reduces once per `M`/`H` entry.
/// Measured on the 64x64 kernel bench (`benches/kernel_tiers.rs`,
/// `wiener_compute_stats_win{5,7}_64x64`): see `docs/perf-status.md`.
#[cfg(target_arch = "aarch64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn compute_stats_impl_neon(
    token: NeonToken,
    wiener_win: usize,
    dgd: &[u8],
    dgd_origin: usize,
    dgd_stride: usize,
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    h_start: i32,
    h_end: i32,
    v_start: i32,
    v_end: i32,
    m: &mut [i64],
    h: &mut [i64],
) {
    let win2 = wiener_win * wiener_win;
    assert!(m.len() >= win2 && h.len() >= win2 * win2);
    let width = (h_end - h_start).max(0) as usize;
    let height = (v_end - v_start).max(0) as usize;
    if !cs_accepts(wiener_win, width, height) {
        compute_stats_scalar_core(
            wiener_win, dgd, dgd_origin, dgd_stride, src, src_origin, src_stride, h_start, h_end,
            v_start, v_end, m, h,
        );
        return;
    }
    let avg = cs_find_average_neon(
        token, dgd, dgd_origin, dgd_stride, h_start, h_end, v_start, v_end,
    ) as i16;
    let g = CsGeom::new(wiener_win, width, height);
    let mut scratch = StatsScratch::take3(g.d_len(), g.s_len(), g.t_len());
    let (d, s, t) = scratch.split3();
    cs_prepare(
        &g, avg, dgd, dgd_origin, dgd_stride, src, src_origin, src_stride, h_start, v_start, d, s,
        t,
    );
    let m = &mut m[..win2];
    let h = &mut h[..win2 * win2];
    if wiener_win == WIENER_WIN_CHROMA {
        cs_kernel!(token, 5, &g, d, s, t, m, h;
            cs_load_neon, cs_and_neon, cs_zero_neon, cs_madd_neon, cs_msub_neon, cs_reduce_neon, cs_mask_neon);
    } else {
        cs_kernel!(token, 7, &g, d, s, t, m, h;
            cs_load_neon, cs_and_neon, cs_zero_neon, cs_madd_neon, cs_msub_neon, cs_reduce_neon, cs_mask_neon);
    }
    cs_mirror(win2, h);
}

/// Per-thread sub-average scratch for the SIMD `compute_stats` arms.
///
/// One `Vec<i16>` holding `d`, `s` and the transposed strips `t` back to
/// back (see [`CsGeom`]), grown to the largest restoration unit seen and
/// never shrunk. `take3` hands out a guard so the
/// buffer returns to the thread slot on drop even on an early return; if the
/// slot is already borrowed (re-entrancy, which does not happen today) the
/// guard owns a fresh allocation instead of panicking.
struct StatsScratch {
    buf: alloc::vec::Vec<i16>,
    dlen: usize,
    slen: usize,
    #[cfg(feature = "std")]
    pooled: bool,
}

#[cfg(feature = "std")]
std::thread_local! {
    static STATS_SCRATCH: core::cell::RefCell<alloc::vec::Vec<i16>> =
        const { core::cell::RefCell::new(alloc::vec::Vec::new()) };
}

impl StatsScratch {
    fn take3(dlen: usize, slen: usize, tlen: usize) -> Self {
        let need = dlen + slen + tlen;
        #[cfg(feature = "std")]
        {
            if let Some(mut buf) = STATS_SCRATCH.with(|c| {
                c.try_borrow_mut()
                    .ok()
                    .map(|mut b| core::mem::take(&mut *b))
            }) {
                if buf.len() < need {
                    buf.resize(need, 0);
                }
                return StatsScratch {
                    buf,
                    dlen,
                    slen,
                    pooled: true,
                };
            }
        }
        StatsScratch {
            buf: alloc::vec![0i16; need],
            dlen,
            slen,
            #[cfg(feature = "std")]
            pooled: false,
        }
    }

    fn split3(&mut self) -> (&mut [i16], &mut [i16], &mut [i16]) {
        let (d, rest) = self.buf.split_at_mut(self.dlen);
        let (s, t) = rest.split_at_mut(self.slen);
        (d, s, t)
    }
}

#[cfg(feature = "std")]
impl Drop for StatsScratch {
    fn drop(&mut self) {
        if self.pooled {
            let buf = core::mem::take(&mut self.buf);
            STATS_SCRATCH.with(|c| {
                if let Ok(mut slot) = c.try_borrow_mut() {
                    *slot = buf;
                }
            });
        }
    }
}

/// Scalar reference — verbatim `svt_av1_compute_stats_c`. The M and H
/// accumulation order below is the byte-exactness anchor for every SIMD tier.
#[allow(clippy::too_many_arguments)]
fn compute_stats_scalar_core(
    wiener_win: usize,
    dgd: &[u8],
    dgd_origin: usize,
    dgd_stride: usize,
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    h_start: i32,
    h_end: i32,
    v_start: i32,
    v_end: i32,
    m: &mut [i64],
    h: &mut [i64],
) {
    let win2 = wiener_win * wiener_win;
    let halfwin = (wiener_win >> 1) as i32;
    assert!(m.len() >= win2 && h.len() >= win2 * win2);
    let avg = find_average(dgd, dgd_origin, dgd_stride, h_start, h_end, v_start, v_end) as i16;

    m[..win2].fill(0);
    h[..win2 * win2].fill(0);
    // Re-slice M and H to their exact working lengths so the hot
    // accumulation below carries no interior bounds checks and LLVM can
    // vectorise the multiply-accumulate. Byte-inert: identical products,
    // and each H/M element is still touched exactly once per pixel in the
    // same (i, j) order, so the i64 accumulation is bit-for-bit unchanged.
    let m = &mut m[..win2];
    let h = &mut h[..win2 * win2];
    let mut y = [0i16; WIENER_WIN * WIENER_WIN];
    for i in v_start..v_end {
        for j in h_start..h_end {
            let sidx = src_origin as isize + i as isize * src_stride as isize + j as isize;
            let x = src[sidx as usize] as i16 - avg;
            let mut idx = 0usize;
            for k in -halfwin..=halfwin {
                for l in -halfwin..=halfwin {
                    let didx = dgd_origin as isize
                        + (i + l) as isize * dgd_stride as isize
                        + (j + k) as isize;
                    y[idx] = dgd[didx as usize] as i16 - avg;
                    idx += 1;
                }
            }
            debug_assert_eq!(idx, win2);
            let ys = &y[..win2];
            let xi = x as i32;
            // Upper-triangular H (`H[k*win2 + l] += y[k]*y[l]` for l >= k)
            // plus `M[k] += y[k]*x`, walked as exact-length chunk/zip pairs.
            // `h` is win2 rows of win2 (chunks_exact_mut leaves no remainder),
            // so `k` ranges 0..win2 and the inner zip is bounds-check-free.
            for (k, hrow) in h.chunks_exact_mut(win2).enumerate() {
                let yk = ys[k] as i32;
                m[k] += (yk * xi) as i64;
                for (hv, &yl) in hrow[k..].iter_mut().zip(&ys[k..]) {
                    *hv += (yk * yl as i32) as i64;
                }
            }
        }
    }
    for k in 0..win2 {
        for l in (k + 1)..win2 {
            h[l * win2 + k] = h[k * win2 + l];
        }
    }
}

/// AVX2 `compute_stats` — C's `svt_av1_compute_stats_avx2` shape
/// (`ASM_AVX2/pickrst_avx2.c:2345`), shared with the NEON arm through
/// [`cs_kernel!`]. See that macro's doc for the six steps and the exactness
/// argument; this function is the per-ISA envelope: sub-average scratch,
/// window-size dispatch, and the mirror.
#[cfg(target_arch = "x86_64")]
#[arcane]
#[allow(clippy::too_many_arguments)]
fn compute_stats_impl_v3(
    token: Desktop64,
    wiener_win: usize,
    dgd: &[u8],
    dgd_origin: usize,
    dgd_stride: usize,
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    h_start: i32,
    h_end: i32,
    v_start: i32,
    v_end: i32,
    m: &mut [i64],
    h: &mut [i64],
) {
    let win2 = wiener_win * wiener_win;
    assert!(m.len() >= win2 && h.len() >= win2 * win2);
    let width = (h_end - h_start).max(0) as usize;
    let height = (v_end - v_start).max(0) as usize;
    if !cs_accepts(wiener_win, width, height) {
        compute_stats_scalar_core(
            wiener_win, dgd, dgd_origin, dgd_stride, src, src_origin, src_stride, h_start, h_end,
            v_start, v_end, m, h,
        );
        return;
    }
    let avg = cs_find_average_v3(
        token, dgd, dgd_origin, dgd_stride, h_start, h_end, v_start, v_end,
    ) as i16;
    let g = CsGeom::new(wiener_win, width, height);
    let mut scratch = StatsScratch::take3(g.d_len(), g.s_len(), g.t_len());
    let (d, s, t) = scratch.split3();
    cs_prepare(
        &g, avg, dgd, dgd_origin, dgd_stride, src, src_origin, src_stride, h_start, v_start, d, s,
        t,
    );
    let m = &mut m[..win2];
    let h = &mut h[..win2 * win2];
    if wiener_win == WIENER_WIN_CHROMA {
        cs_kernel!(token, 5, &g, d, s, t, m, h;
            cs_load_v3, cs_and_v3, cs_zero_v3, cs_madd_v3, cs_msub_v3, cs_reduce_v3, cs_mask_v3);
    } else {
        cs_kernel!(token, 7, &g, d, s, t, m, h;
            cs_load_v3, cs_and_v3, cs_zero_v3, cs_madd_v3, cs_msub_v3, cs_reduce_v3, cs_mask_v3);
    }
    cs_mirror(win2, h);
}

/// WIENER_TAP_SCALE_FACTOR (restoration_pick.c:31).
const WIENER_TAP_SCALE_FACTOR: i64 = 1 << 16;

/// C `wrap_index` (restoration_pick.c:745).
#[inline]
fn wrap_index(i: usize, wiener_win: usize) -> usize {
    let halfwin1 = (wiener_win >> 1) + 1;
    if i >= halfwin1 { wiener_win - 1 - i } else { i }
}

/// C `linsolve_wiener` (restoration_pick.c:752). Returns false when singular.
fn linsolve_wiener(n: usize, a: &mut [i64], stride: usize, b: &mut [i64], x: &mut [i32]) -> bool {
    for k in 0..n.saturating_sub(1) {
        // Partial pivoting
        for i in (k + 1..n).rev() {
            if a[(i - 1) * stride + k].abs() < a[i * stride + k].abs() {
                for j in 0..n {
                    a.swap(i * stride + j, (i - 1) * stride + j);
                }
                b.swap(i, i - 1);
            }
        }
        // Forward elimination
        for i in k..n - 1 {
            if a[k * stride + k] == 0 {
                return false;
            }
            let c = a[(i + 1) * stride + k];
            let cd = a[k * stride + k];
            for j in 0..n {
                // C: A[(i+1)*stride+j] -= c / 256 * A[k*stride+j] / cd * 256;
                a[(i + 1) * stride + j] -= c / 256 * a[k * stride + j] / cd * 256;
            }
            b[i + 1] -= c * b[k] / cd;
        }
    }
    // Back-substitution
    for i in (0..n).rev() {
        if a[i * stride + i] == 0 {
            return false;
        }
        let mut c: i64 = 0;
        for j in (i + 1)..n {
            c += a[i * stride + j] * x[j] as i64 / WIENER_TAP_SCALE_FACTOR;
        }
        x[i] = (WIENER_TAP_SCALE_FACTOR * (b[i] - c) / a[i * stride + i]) as i32;
    }
    true
}

/// C `update_a_sep_sym` (restoration_pick.c:798). Fixes `b`, updates `a`.
fn update_a_sep_sym(wiener_win: usize, m: &[i64], h: &[i64], a: &mut [i32], b: &[i32]) {
    let win2 = wiener_win * wiener_win;
    let halfwin1 = (wiener_win >> 1) + 1;
    let mut av = [0i64; WIENER_HALFWIN + 1];
    let mut bv = [0i64; (WIENER_HALFWIN + 1) * (WIENER_HALFWIN + 1)];

    for i in 0..wiener_win {
        for j in 0..wiener_win {
            let jj = wrap_index(j, wiener_win);
            // Mc[i][j] = M[i*win + j]
            av[jj] += m[i * wiener_win + j] * b[i] as i64 / WIENER_TAP_SCALE_FACTOR;
        }
    }
    for i in 0..wiener_win {
        for j in 0..wiener_win {
            for k in 0..wiener_win {
                for l in 0..wiener_win {
                    let kk = wrap_index(k, wiener_win);
                    let ll = wrap_index(l, wiener_win);
                    // hc[j*win + i] = H + j*win*win2 + i*win; element [k*win2 + l]
                    let hv = h[j * wiener_win * win2 + i * wiener_win + k * win2 + l];
                    bv[ll * halfwin1 + kk] += hv * b[i] as i64 / WIENER_TAP_SCALE_FACTOR
                        * b[j] as i64
                        / WIENER_TAP_SCALE_FACTOR;
                }
            }
        }
    }
    normalize_and_solve(wiener_win, halfwin1, &mut av, &mut bv, a);
}

/// C `update_b_sep_sym` (restoration_pick.c:850). Fixes `a`, updates `b`.
fn update_b_sep_sym(wiener_win: usize, m: &[i64], h: &[i64], a: &[i32], b: &mut [i32]) {
    let win2 = wiener_win * wiener_win;
    let halfwin1 = (wiener_win >> 1) + 1;
    let mut av = [0i64; WIENER_HALFWIN + 1];
    let mut bv = [0i64; (WIENER_HALFWIN + 1) * (WIENER_HALFWIN + 1)];

    for i in 0..wiener_win {
        let ii = wrap_index(i, wiener_win);
        for j in 0..wiener_win {
            av[ii] += m[i * wiener_win + j] * a[j] as i64 / WIENER_TAP_SCALE_FACTOR;
        }
    }
    for i in 0..wiener_win {
        for j in 0..wiener_win {
            let ii = wrap_index(i, wiener_win);
            let jj = wrap_index(j, wiener_win);
            for k in 0..wiener_win {
                for l in 0..wiener_win {
                    // hc[i*win + j] = H + i*win*win2 + j*win; element [k*win2 + l]
                    let hv = h[i * wiener_win * win2 + j * wiener_win + k * win2 + l];
                    bv[jj * halfwin1 + ii] += hv * a[k] as i64 / WIENER_TAP_SCALE_FACTOR
                        * a[l] as i64
                        / WIENER_TAP_SCALE_FACTOR;
                }
            }
        }
    }
    normalize_and_solve(wiener_win, halfwin1, &mut av, &mut bv, b);
}

/// Shared tail of update_{a,b}_sep_sym: normalization enforcement + solve +
/// symmetric expansion (restoration_pick.c:826-846 / 878-898).
fn normalize_and_solve(
    wiener_win: usize,
    halfwin1: usize,
    av: &mut [i64],
    bv: &mut [i64],
    out: &mut [i32],
) {
    let a_halfwin_1 = av[halfwin1 - 1];
    for i in 0..halfwin1 - 1 {
        av[i] -= a_halfwin_1 * 2 + bv[i * halfwin1 + halfwin1 - 1]
            - 2 * bv[(halfwin1 - 1) * halfwin1 + (halfwin1 - 1)];
    }
    for i in 0..halfwin1 - 1 {
        for j in 0..halfwin1 - 1 {
            bv[i * halfwin1 + j] -= 2
                * (bv[i * halfwin1 + (halfwin1 - 1)] + bv[(halfwin1 - 1) * halfwin1 + j]
                    - 2 * bv[(halfwin1 - 1) * halfwin1 + (halfwin1 - 1)]);
        }
    }
    let mut s = [0i32; WIENER_WIN];
    if linsolve_wiener(halfwin1 - 1, bv, halfwin1, av, &mut s) {
        s[halfwin1 - 1] = WIENER_TAP_SCALE_FACTOR as i32;
        for i in halfwin1..wiener_win {
            s[i] = s[wiener_win - 1 - i];
            s[halfwin1 - 1] -= 2 * s[i];
        }
        out[..wiener_win].copy_from_slice(&s[..wiener_win]);
    }
}

/// C `wiener_decompose_sep_sym` (restoration_pick.c:901): 4 alternating
/// update rounds from the mid-tap starting point.
pub fn wiener_decompose_sep_sym(
    wiener_win: usize,
    m: &[i64],
    h: &[i64],
    a: &mut [i32],
    b: &mut [i32],
) {
    const INIT_FILT: [i32; WIENER_WIN] = [
        WIENER_FILT_TAP0_MIDV,
        WIENER_FILT_TAP1_MIDV,
        WIENER_FILT_TAP2_MIDV,
        WIENER_FILT_STEP
            - 2 * (WIENER_FILT_TAP0_MIDV + WIENER_FILT_TAP1_MIDV + WIENER_FILT_TAP2_MIDV),
        WIENER_FILT_TAP2_MIDV,
        WIENER_FILT_TAP1_MIDV,
        WIENER_FILT_TAP0_MIDV,
    ];
    let plane_off = (WIENER_WIN - wiener_win) >> 1;
    for i in 0..wiener_win {
        let v =
            (WIENER_TAP_SCALE_FACTOR / WIENER_FILT_STEP as i64) as i32 * INIT_FILT[i + plane_off];
        a[i] = v;
        b[i] = v;
    }
    // NUM_WIENER_ITERS = 5; iter starts at 1 -> 4 rounds.
    for _ in 1..5 {
        update_a_sep_sym(wiener_win, m, h, a, b);
        update_b_sep_sym(wiener_win, m, h, a, b);
    }
}

/// C `finalize_sym_filter` (restoration_pick.c:973): quantize taps to
/// WIENER_FILT_STEP scale, clamp, mirror, derive the center tap.
pub fn finalize_sym_filter(wiener_win: usize, f: &[i32], fi: &mut [i16; 8]) {
    let halfwin = wiener_win >> 1;
    for i in 0..halfwin {
        let dividend = f[i] as i64 * WIENER_FILT_STEP as i64;
        let divisor = WIENER_TAP_SCALE_FACTOR;
        fi[i] = if dividend < 0 {
            ((dividend - divisor / 2) / divisor) as i16
        } else {
            ((dividend + divisor / 2) / divisor) as i16
        };
    }
    if wiener_win == WIENER_WIN {
        fi[0] = fi[0].clamp(WIENER_FILT_TAP0_MINV as i16, WIENER_FILT_TAP0_MAXV as i16);
        fi[1] = fi[1].clamp(WIENER_FILT_TAP1_MINV as i16, WIENER_FILT_TAP1_MAXV as i16);
        fi[2] = fi[2].clamp(WIENER_FILT_TAP2_MINV as i16, WIENER_FILT_TAP2_MAXV as i16);
    } else {
        fi[2] = fi[1].clamp(WIENER_FILT_TAP2_MINV as i16, WIENER_FILT_TAP2_MAXV as i16);
        fi[1] = fi[0].clamp(WIENER_FILT_TAP1_MINV as i16, WIENER_FILT_TAP1_MAXV as i16);
        fi[0] = 0;
    }
    // Satisfy filter constraints (mirror) + implicit-128 center tap.
    fi[WIENER_WIN - 1] = fi[0];
    fi[WIENER_WIN - 2] = fi[1];
    fi[WIENER_WIN - 3] = fi[2];
    fi[3] = -2 * (fi[0] + fi[1] + fi[2]);
    // C leaves index 7 at its memset-zero value; make that explicit.
    fi[7] = 0;
}

/// C `compute_score` (restoration_pick.c:934): x'Ax - 2x'b of the solved
/// filter minus the identity filter; > 0 means the filter should revert.
pub fn compute_score(
    wiener_win: usize,
    m: &[i64],
    h: &[i64],
    vfilt: &[i16; 8],
    hfilt: &[i16; 8],
) -> i64 {
    let mut a = [0i16; WIENER_WIN];
    let mut b = [0i16; WIENER_WIN];
    let plane_off = (WIENER_WIN - wiener_win) >> 1;
    let win2 = wiener_win * wiener_win;

    a[WIENER_HALFWIN] = WIENER_FILT_STEP as i16;
    b[WIENER_HALFWIN] = WIENER_FILT_STEP as i16;
    for i in 0..WIENER_HALFWIN {
        a[i] = vfilt[i];
        a[WIENER_WIN - i - 1] = vfilt[i];
        b[i] = hfilt[i];
        b[WIENER_WIN - i - 1] = hfilt[i];
        a[WIENER_HALFWIN] -= 2 * a[i];
        b[WIENER_HALFWIN] -= 2 * b[i];
    }
    let mut ab = [0i32; WIENER_WIN * WIENER_WIN];
    for k in 0..wiener_win {
        for l in 0..wiener_win {
            ab[k * wiener_win + l] = a[l + plane_off] as i32 * b[k + plane_off] as i32;
        }
    }
    let mut p: i64 = 0;
    let mut q: i64 = 0;
    for k in 0..win2 {
        p += ab[k] as i64 * m[k] / WIENER_FILT_STEP as i64 / WIENER_FILT_STEP as i64;
        for l in 0..win2 {
            q += ab[k] as i64 * h[k * win2 + l] * ab[l] as i64
                / WIENER_FILT_STEP as i64
                / WIENER_FILT_STEP as i64
                / WIENER_FILT_STEP as i64
                / WIENER_FILT_STEP as i64;
        }
    }
    let score = q - 2 * p;

    let i_p = m[win2 >> 1];
    let i_q = h[(win2 >> 1) * win2 + (win2 >> 1)];
    let i_score = i_q - 2 * i_p;

    score - i_score
}

/// C `svt_extend_frame` / `extend_frame_lowbd` (restoration.c:110):
/// replicate `border_horz`/`border_vert` pixels around the `width x height`
/// crop at `origin`. The plane buffer must physically contain the border.
///
/// Generic over the pixel type: C has two byte-identical bodies
/// (`extend_frame_lowbd` / `extend_frame_highbd`, restoration.c:150-157)
/// differing only in element type — this is one function serving both.
pub fn extend_frame<T: Copy>(
    data: &mut [T],
    origin: usize,
    width: usize,
    height: usize,
    stride: usize,
    border_horz: usize,
    border_vert: usize,
) {
    for i in 0..height {
        let row = origin + i * stride;
        let left = data[row];
        let right = data[row + width - 1];
        data[row - border_horz..row].fill(left);
        data[row + width..row + width + border_horz].fill(right);
    }
    let full_w = width + 2 * border_horz;
    let top_row = origin - border_horz;
    for i in 1..=border_vert {
        let (dst_start, src_start) = (top_row - i * stride, top_row);
        data.copy_within(src_start..src_start + full_w, dst_start);
    }
    let bottom_row = origin - border_horz + (height - 1) * stride;
    for i in 1..=border_vert {
        let dst_start = bottom_row + i * stride;
        data.copy_within(bottom_row..bottom_row + full_w, dst_start);
    }
}

/// C `RestorationTileLimits` (restoration.h:259).
#[derive(Clone, Copy, Debug)]
pub struct TileLimits {
    pub h_start: i32,
    pub h_end: i32,
    pub v_start: i32,
    pub v_end: i32,
}

/// C `Av1PixelRect` (restoration.h:193).
#[derive(Clone, Copy, Debug)]
pub struct PixelRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// C `RestorationStripeBoundaries` (restoration.h:217) at either bit depth.
/// Buffer column `i` corresponds to plane column `i - RESTORATION_EXTRA_HORZ`;
/// row `RESTORATION_CTX_VERT * frame_stripe + j` holds the j-th saved line
/// of that stripe's boundary.
///
/// C keeps ONE `uint8_t*` buffer and scales every byte offset by
/// `<< use_highbd` (restoration.c:249-400, :1492-1597); in PIXEL units the
/// two depths are the same walk, which is what the type parameter expresses.
/// `stride` is in pixels.
#[derive(Clone, Debug, Default)]
pub struct StripeBoundariesT<T> {
    pub above: alloc::vec::Vec<T>,
    pub below: alloc::vec::Vec<T>,
    pub stride: usize,
}

/// The 8-bit boundaries (unchanged name for every existing caller).
pub type StripeBoundaries = StripeBoundariesT<u8>;

/// C `get_stripe_boundary_info` (restoration.c:216).
fn get_stripe_boundary_info(limits: &TileLimits, tile_rect: &PixelRect, ss_y: i32) -> (bool, bool) {
    let mut copy_above = true;
    let mut copy_below = true;

    let full_stripe_height = RESTORATION_PROC_UNIT_SIZE >> ss_y;
    let runit_offset = RESTORATION_UNIT_OFFSET >> ss_y;

    let first_stripe_in_tile = limits.v_start == tile_rect.top;
    let this_stripe_height = full_stripe_height
        - if first_stripe_in_tile {
            runit_offset
        } else {
            0
        };
    let last_stripe_in_tile = limits.v_start + this_stripe_height >= tile_rect.bottom;

    if first_stripe_in_tile {
        copy_above = false;
    }
    if last_stripe_in_tile {
        copy_below = false;
    }
    (copy_above, copy_below)
}

/// Line save/restore scratch — C `RestorationLineBuffers` (restoration.h:206),
/// boundary rows only (the cdef/lr column buffers are unused in the
/// single-tile path). C sizes the rows in BYTES (`tmp_save_above[..][RESTORATION_LINEBUFFER_WIDTH]`,
/// 400 = `2 * RESTORATION_PROC_UNIT_SIZE * 2 + ...` at highbd), so 400 pixels
/// per row covers both depths.
struct LineBuffers<T> {
    above: [[T; 400]; RESTORATION_BORDER as usize],
    below: [[T; 400]; RESTORATION_BORDER as usize],
}

impl<T: Copy + Default> LineBuffers<T> {
    fn new() -> Self {
        LineBuffers {
            above: [[T::default(); 400]; RESTORATION_BORDER as usize],
            below: [[T::default(); 400]; RESTORATION_BORDER as usize],
        }
    }
}

/// C `setup_processing_stripe_boundary` (restoration.c:249), opt=0, at either
/// bit depth (C's `use_highbd` only rescales the byte counts; every offset here
/// is in pixels, so one body serves both).
#[allow(clippy::too_many_arguments)]
fn setup_processing_stripe_boundary<T: Copy>(
    limits: &TileLimits,
    rsb: &StripeBoundariesT<T>,
    rsb_row: i32,
    h: i32,
    data: &mut [T],
    data_origin: usize,
    data_stride: usize,
    rlbs: &mut LineBuffers<T>,
    copy_above: bool,
    copy_below: bool,
) {
    let buf_stride = rsb.stride as i32;
    let buf_x0_off = limits.h_start;
    let line_width = (limits.h_end - limits.h_start) + 2 * RESTORATION_EXTRA_HORZ;
    let line_size = line_width as usize;

    let data_x0 = limits.h_start - RESTORATION_EXTRA_HORZ;

    if copy_above {
        let data_tl = data_origin as isize
            + data_x0 as isize
            + limits.v_start as isize * data_stride as isize;
        for i in -RESTORATION_BORDER..0 {
            let buf_row = rsb_row + (i + RESTORATION_CTX_VERT).max(0);
            let buf_off = (buf_x0_off + buf_row * buf_stride) as usize;
            let dst = (data_tl + i as isize * data_stride as isize) as usize;
            rlbs.above[(i + RESTORATION_BORDER) as usize][..line_size]
                .copy_from_slice(&data[dst..dst + line_size]);
            data[dst..dst + line_size].copy_from_slice(&rsb.above[buf_off..buf_off + line_size]);
        }
    }
    if copy_below {
        let stripe_end = limits.v_start + h;
        let data_bl =
            data_origin as isize + data_x0 as isize + stripe_end as isize * data_stride as isize;
        for i in 0..RESTORATION_BORDER {
            let buf_row = rsb_row + i.min(RESTORATION_CTX_VERT - 1);
            let buf_off = (buf_x0_off + buf_row * buf_stride) as usize;
            let dst = (data_bl + i as isize * data_stride as isize) as usize;
            rlbs.below[i as usize][..line_size].copy_from_slice(&data[dst..dst + line_size]);
            data[dst..dst + line_size].copy_from_slice(&rsb.below[buf_off..buf_off + line_size]);
        }
    }
}

/// C `restore_processing_stripe_boundary` (restoration.c:347), opt=0, at
/// either bit depth (same pixel-unit walk as the setup above).
#[allow(clippy::too_many_arguments)]
fn restore_processing_stripe_boundary<T: Copy>(
    limits: &TileLimits,
    rlbs: &LineBuffers<T>,
    h: i32,
    data: &mut [T],
    data_origin: usize,
    data_stride: usize,
    copy_above: bool,
    copy_below: bool,
) {
    let line_width = (limits.h_end - limits.h_start) + 2 * RESTORATION_EXTRA_HORZ;
    let line_size = line_width as usize;
    let data_x0 = limits.h_start - RESTORATION_EXTRA_HORZ;

    if copy_above {
        let data_tl = data_origin as isize
            + data_x0 as isize
            + limits.v_start as isize * data_stride as isize;
        for i in -RESTORATION_BORDER..0 {
            let dst = (data_tl + i as isize * data_stride as isize) as usize;
            data[dst..dst + line_size]
                .copy_from_slice(&rlbs.above[(i + RESTORATION_BORDER) as usize][..line_size]);
        }
    }
    if copy_below {
        let stripe_bottom = limits.v_start + h;
        let data_bl =
            data_origin as isize + data_x0 as isize + stripe_bottom as isize * data_stride as isize;
        for i in 0..RESTORATION_BORDER {
            if stripe_bottom + i >= limits.v_end + RESTORATION_BORDER {
                break;
            }
            let dst = (data_bl + i as isize * data_stride as isize) as usize;
            data[dst..dst + line_size].copy_from_slice(&rlbs.below[i as usize][..line_size]);
        }
    }
}

/// C `wiener_filter_stripe` (restoration.c:399): proc-unit column loop with
/// the 16-px width round-up.
#[allow(clippy::too_many_arguments)]
fn wiener_filter_stripe(
    wiener: &WienerInfo,
    stripe_width: i32,
    stripe_height: i32,
    procunit_width: i32,
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_origin: usize,
    dst_stride: usize,
) {
    let mut j = 0i32;
    while j < stripe_width {
        let w = procunit_width.min((stripe_width - j + 15) & !15);
        wiener_convolve_add_src(
            src,
            src_origin + j as usize,
            src_stride,
            dst,
            dst_origin + j as usize,
            dst_stride,
            &wiener.hfilter,
            &wiener.vfilter,
            w as usize,
            stripe_height as usize,
        );
        j += procunit_width;
    }
}

/// The one per-depth kernel the unit filter needs: C's
/// `wiener_filter_stripe` / `wiener_filter_stripe_highbd` split
/// (restoration.c:399 / :987). Private — the public surface is the two typed
/// entry points below.
trait WienerStripePixel: Copy + Default {
    #[allow(clippy::too_many_arguments)]
    fn filter_stripe(
        wiener: &WienerInfo,
        stripe_width: i32,
        stripe_height: i32,
        procunit_width: i32,
        src: &[Self],
        src_origin: usize,
        src_stride: usize,
        dst: &mut [Self],
        dst_origin: usize,
        dst_stride: usize,
        bd: i32,
    );

    /// `sgrproj_filter_stripe` / `sgrproj_filter_stripe_highbd`
    /// (restoration.c:964 / :1010) — the second entry of C's
    /// `stripe_filters[]` table, selected by
    /// `2 * highbd + (unit_rtype == RESTORE_SGRPROJ)`.
    #[allow(clippy::too_many_arguments)]
    fn filter_stripe_sgr(
        ep: usize,
        xqd: &[i32; 2],
        stripe_width: i32,
        stripe_height: i32,
        procunit_width: i32,
        src: &[Self],
        src_origin: usize,
        src_stride: usize,
        dst: &mut [Self],
        dst_origin: usize,
        dst_stride: usize,
        bd: i32,
    );
}

impl WienerStripePixel for u8 {
    fn filter_stripe(
        wiener: &WienerInfo,
        stripe_width: i32,
        stripe_height: i32,
        procunit_width: i32,
        src: &[u8],
        src_origin: usize,
        src_stride: usize,
        dst: &mut [u8],
        dst_origin: usize,
        dst_stride: usize,
        _bd: i32,
    ) {
        wiener_filter_stripe(
            wiener,
            stripe_width,
            stripe_height,
            procunit_width,
            src,
            src_origin,
            src_stride,
            dst,
            dst_origin,
            dst_stride,
        );
    }

    fn filter_stripe_sgr(
        ep: usize,
        xqd: &[i32; 2],
        stripe_width: i32,
        stripe_height: i32,
        procunit_width: i32,
        src: &[u8],
        src_origin: usize,
        src_stride: usize,
        dst: &mut [u8],
        dst_origin: usize,
        dst_stride: usize,
        _bd: i32,
    ) {
        crate::port_sgr::sgrproj_filter_stripe(
            ep,
            xqd,
            stripe_width,
            stripe_height,
            procunit_width,
            src,
            src_origin,
            src_stride,
            dst,
            dst_origin,
            dst_stride,
        );
    }
}

impl WienerStripePixel for u16 {
    fn filter_stripe(
        wiener: &WienerInfo,
        stripe_width: i32,
        stripe_height: i32,
        procunit_width: i32,
        src: &[u16],
        src_origin: usize,
        src_stride: usize,
        dst: &mut [u16],
        dst_origin: usize,
        dst_stride: usize,
        bd: i32,
    ) {
        wiener_filter_stripe_hbd(
            wiener,
            stripe_width,
            stripe_height,
            procunit_width,
            src,
            src_origin,
            src_stride,
            dst,
            dst_origin,
            dst_stride,
            bd,
        );
    }

    fn filter_stripe_sgr(
        ep: usize,
        xqd: &[i32; 2],
        stripe_width: i32,
        stripe_height: i32,
        procunit_width: i32,
        src: &[u16],
        src_origin: usize,
        src_stride: usize,
        dst: &mut [u16],
        dst_origin: usize,
        dst_stride: usize,
        bd: i32,
    ) {
        crate::port_sgr::sgrproj_filter_stripe_highbd(
            ep,
            xqd,
            stripe_width,
            stripe_height,
            procunit_width,
            src,
            src_origin,
            src_stride,
            dst,
            dst_origin,
            dst_stride,
            bd,
        );
    }
}

/// C `svt_av1_loop_restoration_filter_unit` (restoration.c:1040), 8-bit.
/// Dispatches C's `filter_idx = 2 * highbd + (unit_rtype == RESTORE_SGRPROJ)`;
/// the SGR arm is live on the VIDEO path at presets 0..3 (it is `sg_filter_lvl
/// = 0`, hence unreachable, on the all-intra one).
///
/// `data`/`dst` are padded planes; `*_origin` indexes plane (0,0). `data` is
/// temporarily modified around stripe boundaries when `need_boundaries` is
/// set (decoder-exact application); the search path passes false
/// (`use_boundaries_in_rest_search = 0`, enc_handle.c:4483).
#[allow(clippy::too_many_arguments)]
pub fn loop_restoration_filter_unit(
    need_boundaries: bool,
    limits: &TileLimits,
    rui: &RestUnitParams,
    rsb: &StripeBoundaries,
    tile_rect: &PixelRect,
    tile_stripe0: i32,
    ss_x: i32,
    ss_y: i32,
    data: &mut [u8],
    data_origin: usize,
    stride: usize,
    dst: &mut [u8],
    dst_origin: usize,
    dst_stride: usize,
) {
    filter_unit_impl(
        need_boundaries,
        limits,
        rui,
        rsb,
        tile_rect,
        tile_stripe0,
        ss_x,
        ss_y,
        data,
        data_origin,
        stride,
        dst,
        dst_origin,
        dst_stride,
        8,
    );
}

/// C `svt_av1_loop_restoration_filter_unit` (restoration.c:1040) at
/// `highbd = 1` with BOTH `need_boundaries` arms — the decoder-exact APPLY
/// twin of [`loop_restoration_filter_unit`] for the 10-bit recon (issue #13).
///
/// The stripe split, the boundary save/substitute/restore and the RESTORE_NONE
/// copy are the same pixel-unit walk as at 8 bits (C only rescales byte
/// counts by `use_highbd`); the convolve is `wiener_filter_stripe_highbd`.
/// [`loop_restoration_filter_unit_search_hbd`] remains the boundary-less
/// search arm and is untouched.
#[allow(clippy::too_many_arguments)]
pub fn loop_restoration_filter_unit_hbd(
    need_boundaries: bool,
    limits: &TileLimits,
    rui: &RestUnitParams,
    rsb: &StripeBoundariesT<u16>,
    tile_rect: &PixelRect,
    tile_stripe0: i32,
    ss_x: i32,
    ss_y: i32,
    data: &mut [u16],
    data_origin: usize,
    stride: usize,
    dst: &mut [u16],
    dst_origin: usize,
    dst_stride: usize,
    bd: i32,
) {
    filter_unit_impl(
        need_boundaries,
        limits,
        rui,
        rsb,
        tile_rect,
        tile_stripe0,
        ss_x,
        ss_y,
        data,
        data_origin,
        stride,
        dst,
        dst_origin,
        dst_stride,
        bd,
    );
}

#[allow(clippy::too_many_arguments)]
fn filter_unit_impl<T: WienerStripePixel>(
    need_boundaries: bool,
    limits: &TileLimits,
    rui: &RestUnitParams,
    rsb: &StripeBoundariesT<T>,
    tile_rect: &PixelRect,
    tile_stripe0: i32,
    ss_x: i32,
    ss_y: i32,
    data: &mut [T],
    data_origin: usize,
    stride: usize,
    dst: &mut [T],
    dst_origin: usize,
    dst_stride: usize,
    bd: i32,
) {
    let unit_h = limits.v_end - limits.v_start;
    let unit_w = limits.h_end - limits.h_start;
    let data_tl = data_origin + limits.v_start as usize * stride + limits.h_start as usize;
    let dst_tl = dst_origin + limits.v_start as usize * dst_stride + limits.h_start as usize;

    if rui.rtype == RESTORE_NONE {
        for i in 0..unit_h as usize {
            let s = data_tl + i * stride;
            let d = dst_tl + i * dst_stride;
            let (a, b) = (s..s + unit_w as usize, d..d + unit_w as usize);
            dst[b].copy_from_slice(&data[a]);
        }
        return;
    }
    // C `filter_idx = 2 * highbd + (unit_rtype == RESTORE_SGRPROJ)` — the
    // pixel type carries the `highbd` half, this flag the other.
    debug_assert!(rui.rtype == RESTORE_WIENER || rui.rtype == RESTORE_SGRPROJ);
    let is_sgr = rui.rtype == RESTORE_SGRPROJ;

    let procunit_width = RESTORATION_PROC_UNIT_SIZE >> ss_x;
    let mut rlbs = LineBuffers::<T>::new();

    let mut remaining = *limits;
    let mut i = 0i32;
    while i < unit_h {
        remaining.v_start = limits.v_start + i;
        let (copy_above, copy_below) = get_stripe_boundary_info(&remaining, tile_rect, ss_y);

        let full_stripe_height = RESTORATION_PROC_UNIT_SIZE >> ss_y;
        let runit_offset = RESTORATION_UNIT_OFFSET >> ss_y;

        let tile_stripe = (remaining.v_start - tile_rect.top + runit_offset) / full_stripe_height;
        let frame_stripe = tile_stripe0 + tile_stripe;
        let rsb_row = RESTORATION_CTX_VERT * frame_stripe;

        let nominal_stripe_height =
            full_stripe_height - if tile_stripe == 0 { runit_offset } else { 0 };
        let h = nominal_stripe_height.min(remaining.v_end - remaining.v_start);

        if need_boundaries {
            setup_processing_stripe_boundary(
                &remaining,
                rsb,
                rsb_row,
                h,
                data,
                data_origin,
                stride,
                &mut rlbs,
                copy_above,
                copy_below,
            );
        }
        if is_sgr {
            T::filter_stripe_sgr(
                rui.sgr_ep,
                &rui.sgr_xqd,
                unit_w,
                h,
                procunit_width,
                data,
                data_tl + i as usize * stride,
                stride,
                dst,
                dst_tl + i as usize * dst_stride,
                dst_stride,
                bd,
            );
        } else {
            T::filter_stripe(
                &rui.wiener,
                unit_w,
                h,
                procunit_width,
                data,
                data_tl + i as usize * stride,
                stride,
                dst,
                dst_tl + i as usize * dst_stride,
                dst_stride,
                bd,
            );
        }
        if need_boundaries {
            restore_processing_stripe_boundary(
                &remaining,
                &rlbs,
                h,
                data,
                data_origin,
                stride,
                copy_above,
                copy_below,
            );
        }

        i += h;
    }
}

/// C `count_units_in_tile` (restoration.c:71).
pub fn count_units_in_tile(unit_size: i32, tile_size: i32) -> i32 {
    ((tile_size + (unit_size >> 1)) / unit_size).max(1)
}

/// Iterate restoration units exactly like C `foreach_rest_unit_in_tile`
/// (restoration.c:1227): unit extents with the 150% edge extension and the
/// RESTORATION_UNIT_OFFSET upward shift. Calls `f(limits, unit_idx)`.
pub fn foreach_rest_unit_in_tile(
    tile_rect: &PixelRect,
    hunits_per_tile: i32,
    unit_size: i32,
    ss_y: i32,
    mut f: impl FnMut(&TileLimits, i32),
) {
    let tile_w = tile_rect.right - tile_rect.left;
    let tile_h = tile_rect.bottom - tile_rect.top;
    let ext_size = unit_size * 3 / 2;

    let mut y0 = 0i32;
    let mut i = 0i32;
    while y0 < tile_h {
        let remaining_h = tile_h - y0;
        let h = if remaining_h < ext_size {
            remaining_h
        } else {
            unit_size
        };

        let mut limits = TileLimits {
            h_start: 0,
            h_end: 0,
            v_start: tile_rect.top + y0,
            v_end: tile_rect.top + y0 + h,
        };
        let voffset = RESTORATION_UNIT_OFFSET >> ss_y;
        limits.v_start = tile_rect.top.max(limits.v_start - voffset);
        if limits.v_end < tile_rect.bottom {
            limits.v_end -= voffset;
        }

        let mut x0 = 0i32;
        let mut j = 0i32;
        while x0 < tile_w {
            let remaining_w = tile_w - x0;
            let w = if remaining_w < ext_size {
                remaining_w
            } else {
                unit_size
            };
            limits.h_start = tile_rect.left + x0;
            limits.h_end = tile_rect.left + x0 + w;

            f(&limits, i * hunits_per_tile + j);

            x0 += w;
            j += 1;
        }
        y0 += h;
        i += 1;
    }
}

/// C `extend_lines` (restoration.c:1492); the `use_highbitdepth` arm is the
/// same fill in `uint16_t` units, so one generic body.
fn extend_lines<T: Copy>(
    buf: &mut [T],
    start: usize,
    width: usize,
    height: usize,
    stride: usize,
    extend: usize,
) {
    for i in 0..height {
        let row = start + i * stride;
        let left = buf[row];
        let right = buf[row + width - 1];
        buf[row - extend..row].fill(left);
        buf[row + width..row + width + extend].fill(right);
    }
}

/// C `svt_aom_save_deblock_boundary_lines` (restoration.c:1507), no superres,
/// either bit depth (`use_highbd` there only rescales byte counts).
#[allow(clippy::too_many_arguments)]
fn save_deblock_boundary_lines<T: Copy>(
    src: &[T],
    src_origin: usize,
    src_stride: usize,
    src_width: i32,
    src_height: i32,
    row: i32,
    stripe: i32,
    is_above: bool,
    boundaries: &mut StripeBoundariesT<T>,
) {
    let bdry_buf = if is_above {
        &mut boundaries.above
    } else {
        &mut boundaries.below
    };
    let bdry_stride = boundaries.stride;
    // bdry_start = buf + RESTORATION_EXTRA_HORZ
    let bdry_rows = RESTORATION_EXTRA_HORZ as usize
        + RESTORATION_CTX_VERT as usize * stripe as usize * bdry_stride;

    let lines_to_save = RESTORATION_CTX_VERT.min(src_height - row);
    debug_assert!(lines_to_save == 1 || lines_to_save == 2);

    let upscaled_width = src_width as usize;
    for i in 0..lines_to_save as usize {
        let s = src_origin + (row as usize + i) * src_stride;
        let d = bdry_rows + i * bdry_stride;
        bdry_buf[d..d + upscaled_width].copy_from_slice(&src[s..s + upscaled_width]);
    }
    if lines_to_save == 1 {
        let (a, b) = (bdry_rows, bdry_rows + bdry_stride);
        bdry_buf.copy_within(a..a + upscaled_width, b);
    }
    extend_lines(
        bdry_buf,
        bdry_rows,
        upscaled_width,
        RESTORATION_CTX_VERT as usize,
        bdry_stride,
        RESTORATION_EXTRA_HORZ as usize,
    );
}

/// C `svt_aom_save_cdef_boundary_lines` (restoration.c:1561), no superres,
/// either bit depth.
#[allow(clippy::too_many_arguments)]
fn save_cdef_boundary_lines<T: Copy>(
    src: &[T],
    src_origin: usize,
    src_stride: usize,
    src_width: i32,
    row: i32,
    stripe: i32,
    is_above: bool,
    boundaries: &mut StripeBoundariesT<T>,
) {
    let bdry_buf = if is_above {
        &mut boundaries.above
    } else {
        &mut boundaries.below
    };
    let bdry_stride = boundaries.stride;
    let bdry_rows = RESTORATION_EXTRA_HORZ as usize
        + RESTORATION_CTX_VERT as usize * stripe as usize * bdry_stride;
    let upscaled_width = src_width as usize;
    let s = src_origin + row as usize * src_stride;
    for i in 0..RESTORATION_CTX_VERT as usize {
        let d = bdry_rows + i * bdry_stride;
        bdry_buf[d..d + upscaled_width].copy_from_slice(&src[s..s + upscaled_width]);
    }
    extend_lines(
        bdry_buf,
        bdry_rows,
        upscaled_width,
        RESTORATION_CTX_VERT as usize,
        bdry_stride,
        RESTORATION_EXTRA_HORZ as usize,
    );
}

/// C `svt_aom_save_tile_row_boundary_lines` (restoration.c:1591): one tile
/// row spanning the whole frame. `after_cdef=false` saves deblocked context,
/// `true` saves CDEF context where deblocked context was NOT saved.
///
/// Generic over the pixel type: `u8` is the existing 8-bit path (every caller
/// infers it), `u16` is the highbd twin the 10-bit apply needs (issue #13) —
/// C's `use_highbd` flag only rescales byte counts inside the helpers.
#[allow(clippy::too_many_arguments)]
pub fn save_tile_row_boundary_lines<T: Copy>(
    src: &[T],
    src_origin: usize,
    src_stride: usize,
    src_width: i32,
    src_height: i32,
    ss_y: i32,
    after_cdef: bool,
    boundaries: &mut StripeBoundariesT<T>,
) {
    let stripe_height = RESTORATION_PROC_UNIT_SIZE >> ss_y;
    let stripe_off = RESTORATION_UNIT_OFFSET >> ss_y;
    // whole_frame_rect on this plane
    let tile_rect = PixelRect {
        left: 0,
        top: 0,
        right: src_width,
        bottom: src_height,
    };
    let plane_height = src_height;

    let mut tile_stripe = 0i32;
    loop {
        let rel_y0 = (tile_stripe * stripe_height - stripe_off).max(0);
        let y0 = tile_rect.top + rel_y0;
        if y0 >= tile_rect.bottom {
            break;
        }
        let rel_y1 = (tile_stripe + 1) * stripe_height - stripe_off;
        let y1 = (tile_rect.top + rel_y1).min(tile_rect.bottom);

        let frame_stripe = tile_stripe;
        let use_deblock_above = frame_stripe > 0;
        let use_deblock_below = y1 < plane_height;

        if !after_cdef {
            if use_deblock_above {
                save_deblock_boundary_lines(
                    src,
                    src_origin,
                    src_stride,
                    src_width,
                    src_height,
                    y0 - RESTORATION_CTX_VERT,
                    frame_stripe,
                    true,
                    boundaries,
                );
            }
            if use_deblock_below {
                save_deblock_boundary_lines(
                    src,
                    src_origin,
                    src_stride,
                    src_width,
                    src_height,
                    y1,
                    frame_stripe,
                    false,
                    boundaries,
                );
            }
        } else {
            if !use_deblock_above {
                save_cdef_boundary_lines(
                    src,
                    src_origin,
                    src_stride,
                    src_width,
                    y0,
                    frame_stripe,
                    true,
                    boundaries,
                );
            }
            if !use_deblock_below {
                save_cdef_boundary_lines(
                    src,
                    src_origin,
                    src_stride,
                    src_width,
                    y1 - 1,
                    frame_stripe,
                    false,
                    boundaries,
                );
            }
        }
        tile_stripe += 1;
    }
}

/// Stripe-boundary buffer allocation, C `svt_av1_alloc_restoration_buffers`
/// (restoration.c:1685): rows for `ceil((8 + mi_rows*4) / 64)` stripes at a
/// 32-aligned `plane_w + 8` stride.
pub fn alloc_stripe_boundaries(frame_width: i32, frame_height: i32, ss_x: i32) -> StripeBoundaries {
    alloc_stripe_boundaries_t::<u8>(frame_width, frame_height, ss_x)
}

/// [`alloc_stripe_boundaries`] at any pixel type (`u16` for the 10-bit apply).
/// C allocates `stripe_boundary_size << use_highbd` BYTES (restoration.c:1685-
/// 1700) — the same number of pixels at either depth.
pub fn alloc_stripe_boundaries_t<T: Copy + Default>(
    frame_width: i32,
    frame_height: i32,
    ss_x: i32,
) -> StripeBoundariesT<T> {
    let ext_h = RESTORATION_UNIT_OFFSET + frame_height;
    let num_stripes = (ext_h + 63) / 64;
    let plane_w = ((frame_width + ss_x) >> ss_x) + 2 * RESTORATION_EXTRA_HORZ;
    // ALIGN_POWER_OF_TWO(plane_w, 5)
    let stride = ((plane_w + 31) & !31) as usize;
    let size = num_stripes as usize * stride * RESTORATION_CTX_VERT as usize;
    StripeBoundariesT {
        above: alloc::vec![T::default(); size],
        below: alloc::vec![T::default(); size],
        stride,
    }
}

/// Region SSE (C `svt_aom_get_sse` semantics as used by
/// `sse_restoration_unit`, svt_psnr.c:189).
#[allow(clippy::too_many_arguments)]
pub fn sse_region(
    a: &[u8],
    a_origin: usize,
    a_stride: usize,
    b: &[u8],
    b_origin: usize,
    b_stride: usize,
    width: usize,
    height: usize,
) -> i64 {
    let mut sse: i64 = 0;
    for i in 0..height {
        for j in 0..width {
            let d = a[a_origin + i * a_stride + j] as i64 - b[b_origin + i * b_stride + j] as i64;
            sse += d * d;
        }
    }
    sse
}

// ===========================================================================
// HIGHBD arm — the `is_16bit` (10-bit) loop-restoration SEARCH.
//
// C keeps a parallel highbd implementation of every kernel the Wiener search
// touches, selected by `cm->use_highbitdepth`:
//   sse_restoration_unit -> svt_aom_highbd_get_{y,u,v}_sse_part
//                           (restoration_pick.c:43-51, svt_psnr.c:93)
//   search_wiener_seg    -> svt_av1_compute_stats_highbd
//                           (restoration_pick.c:1332, :692)
//   try_restoration_unit -> svt_av1_loop_restoration_filter_unit(.., highbd=1,
//                           bit_depth) -> wiener_filter_stripe_highbd
//                           -> svt_av1_highbd_wiener_convolve_add_src
//                           (restoration.c, convolve.c:200)
//   svt_extend_frame     -> extend_frame_highbd (restoration.c:152)
// Every one of them is ported below and FFI-pinned in
// tests/c_parity_wiener_hbd.rs. The bd8 kernels above are untouched.
// ===========================================================================

/// C `find_average_highbd` (restoration_pick.h:33) — u16 twin of
/// [`find_average`], returning the u16 mean (C truncates the u64 quotient).
#[allow(clippy::too_many_arguments)]
pub fn find_average_hbd(
    src: &[u16],
    origin: usize,
    stride: usize,
    h_start: i32,
    h_end: i32,
    v_start: i32,
    v_end: i32,
) -> u16 {
    let mut sum: u64 = 0;
    for i in v_start..v_end {
        for j in h_start..h_end {
            let idx = origin as isize + i as isize * stride as isize + j as isize;
            sum += src[idx as usize] as u64;
        }
    }
    (sum / ((v_end - v_start) as u64 * (h_end - h_start) as u64)) as u16
}

/// C `svt_av1_compute_stats_highbd_c` (restoration_pick.c:692).
///
/// Two deltas vs the 8-bit [`compute_stats`], both load-bearing:
/// * the windowed differences are `int32_t` (not `int16_t`) and the products
///   accumulate as `int64_t` — a 10-bit residual overflows the 16-bit form;
/// * every M and H entry is divided by `bit_depth_divider` at the end
///   (4 at EB_TEN_BIT, 16 at EB_TWELVE_BIT) — an integer division applied
///   AFTER accumulation, so it is NOT the same as scaling the inputs.
///   Note C divides the diagonal `H[k][k]` and the upper triangle, then
///   MIRRORS the divided upper triangle down; it never divides the lower
///   triangle separately.
#[allow(clippy::too_many_arguments)]
pub fn compute_stats_hbd(
    wiener_win: usize,
    dgd: &[u16],
    dgd_origin: usize,
    dgd_stride: usize,
    src: &[u16],
    src_origin: usize,
    src_stride: usize,
    h_start: i32,
    h_end: i32,
    v_start: i32,
    v_end: i32,
    m: &mut [i64],
    h: &mut [i64],
    bit_depth: u8,
) {
    let win2 = wiener_win * wiener_win;
    let halfwin = (wiener_win >> 1) as i32;
    assert!(m.len() >= win2 && h.len() >= win2 * win2);
    let avg = find_average_hbd(dgd, dgd_origin, dgd_stride, h_start, h_end, v_start, v_end) as i32;
    let divider: i64 = match bit_depth {
        12 => 16,
        10 => 4,
        _ => 1,
    };

    m[..win2].fill(0);
    h[..win2 * win2].fill(0);
    // Same byte-inert reshaping as the 8-bit [`compute_stats`]: exact-length
    // M/H slices + a check-free chunk/zip walk of the upper-triangular
    // accumulation. Identical i64 products in the same per-element order, so
    // M/H (and thus the divided/mirrored result below) are bit-for-bit equal.
    let m = &mut m[..win2];
    let h = &mut h[..win2 * win2];
    let mut y = [0i32; WIENER_WIN * WIENER_WIN];
    for i in v_start..v_end {
        for j in h_start..h_end {
            let sidx = src_origin as isize + i as isize * src_stride as isize + j as isize;
            let x = src[sidx as usize] as i32 - avg;
            let mut idx = 0usize;
            for k in -halfwin..=halfwin {
                for l in -halfwin..=halfwin {
                    let didx = dgd_origin as isize
                        + (i + l) as isize * dgd_stride as isize
                        + (j + k) as isize;
                    y[idx] = dgd[didx as usize] as i32 - avg;
                    idx += 1;
                }
            }
            debug_assert_eq!(idx, win2);
            let ys = &y[..win2];
            let xi = x as i64;
            for (k, hrow) in h.chunks_exact_mut(win2).enumerate() {
                let yk = ys[k] as i64;
                m[k] += yk * xi;
                for (hv, &yl) in hrow[k..].iter_mut().zip(&ys[k..]) {
                    *hv += yk * yl as i64;
                }
            }
        }
    }
    for k in 0..win2 {
        m[k] /= divider;
        h[k * win2 + k] /= divider;
        for l in (k + 1)..win2 {
            h[k * win2 + l] /= divider;
            h[l * win2 + k] = h[k * win2 + l];
        }
    }
}

/// C `svt_av1_highbd_wiener_convolve_add_src_c` (convolve.c:200) — u16 twin
/// of [`wiener_convolve_add_src`] with a live `bd`. `bd` enters in exactly
/// three places: the horizontal rounding offset `1 << (bd + FILTER_BITS - 1)`,
/// the intermediate clamp `WIENER_CLAMP_LIMIT(round0, bd)`, and the vertical
/// rounding offset `1 << (bd + round1 - 1)` + the final
/// `clip_pixel_highbd(_, bd)`.
///
/// `get_conv_params_wiener(bd)` leaves round_0/round_1 at 3/11 for bd <= 10
/// (`intbufrange = bd + 7 - 3 + 2` only exceeds 16 at bd12), so the shifts
/// are the bd8 ones — asserted below.
#[allow(clippy::too_many_arguments)]
pub fn wiener_convolve_add_src_hbd(
    src: &[u16],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u16],
    dst_origin: usize,
    dst_stride: usize,
    hfilter: &[i16; 8],
    vfilter: &[i16; 8],
    w: usize,
    h: usize,
    bd: i32,
) {
    debug_assert!(
        bd + FILTER_BITS - WIENER_ROUND0_BITS + 2 <= 16,
        "get_conv_params_wiener would re-balance round_0/round_1 above bd10"
    );
    let ih = h + 6;
    let mut temp = alloc::vec![0u16; (ih + 1) * w.max(1)];
    let tstride = w;

    let clamp_limit = (1i32 << (bd + 1 + FILTER_BITS - WIENER_ROUND0_BITS)) - 1;
    for y in 0..ih {
        let row_base = (src_origin + y * src_stride) as isize - 3 * src_stride as isize;
        for x in 0..w {
            let px = |k: usize| -> i32 {
                let idx = row_base + x as isize + k as isize - 3;
                src[idx as usize] as i32
            };
            let mut sum: i32 = (px(3) << FILTER_BITS) + (1 << (bd + FILTER_BITS - 1));
            for (k, &f) in hfilter.iter().enumerate() {
                sum += px(k) * f as i32;
            }
            temp[y * tstride + x] =
                round_power_of_two(sum, WIENER_ROUND0_BITS).clamp(0, clamp_limit) as u16;
        }
    }

    let pixel_max = (1i32 << bd) - 1;
    for x in 0..w {
        for y in 0..h {
            let base = y * tstride + x;
            let center = temp[base + 3 * tstride] as i32;
            let mut sum: i32 = (center << FILTER_BITS) - (1 << (bd + WIENER_ROUND1_BITS - 1));
            for (k, &f) in vfilter.iter().enumerate() {
                sum += temp[base + k * tstride] as i32 * f as i32;
            }
            dst[dst_origin + y * dst_stride + x] =
                round_power_of_two(sum, WIENER_ROUND1_BITS).clamp(0, pixel_max) as u16;
        }
    }
}

/// C `wiener_filter_stripe_highbd` (restoration.c): u16 twin of
/// [`wiener_filter_stripe`].
#[allow(clippy::too_many_arguments)]
fn wiener_filter_stripe_hbd(
    wiener: &WienerInfo,
    stripe_width: i32,
    stripe_height: i32,
    procunit_width: i32,
    src: &[u16],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u16],
    dst_origin: usize,
    dst_stride: usize,
    bd: i32,
) {
    let mut j = 0i32;
    while j < stripe_width {
        let w = procunit_width.min((stripe_width - j + 15) & !15);
        wiener_convolve_add_src_hbd(
            src,
            src_origin + j as usize,
            src_stride,
            dst,
            dst_origin + j as usize,
            dst_stride,
            &wiener.hfilter,
            &wiener.vfilter,
            w as usize,
            stripe_height as usize,
            bd,
        );
        j += procunit_width;
    }
}

/// C `svt_av1_loop_restoration_filter_unit` (restoration.c:1040) at
/// `highbd = 1` and `need_boundaries = 0` — the SEARCH path.
///
/// `use_boundaries_in_rest_search = 0` (enc_handle.c:4483) means
/// `try_restoration_unit_seg` never runs the stripe-boundary save/restore, so
/// this omits that machinery entirely rather than carrying an untested copy
/// of it; the bd8 [`loop_restoration_filter_unit`] keeps both arms because it
/// also serves the decoder-exact APPLY. The stripe SPLIT itself is kept
/// verbatim — it is what makes the filter see 64-row stripes offset by
/// `RESTORATION_UNIT_OFFSET`, which changes the output even with no boundary
/// substitution.
#[allow(clippy::too_many_arguments)]
pub fn loop_restoration_filter_unit_search_hbd(
    limits: &TileLimits,
    rui: &RestUnitParams,
    tile_rect: &PixelRect,
    tile_stripe0: i32,
    ss_x: i32,
    ss_y: i32,
    data: &[u16],
    data_origin: usize,
    stride: usize,
    dst: &mut [u16],
    dst_origin: usize,
    dst_stride: usize,
    bd: i32,
) {
    let unit_h = limits.v_end - limits.v_start;
    let unit_w = limits.h_end - limits.h_start;
    let data_tl = data_origin + limits.v_start as usize * stride + limits.h_start as usize;
    let dst_tl = dst_origin + limits.v_start as usize * dst_stride + limits.h_start as usize;

    if rui.rtype == RESTORE_NONE {
        for i in 0..unit_h as usize {
            let s = data_tl + i * stride;
            let d = dst_tl + i * dst_stride;
            dst[d..d + unit_w as usize].copy_from_slice(&data[s..s + unit_w as usize]);
        }
        return;
    }
    debug_assert!(rui.rtype == RESTORE_WIENER || rui.rtype == RESTORE_SGRPROJ);

    let procunit_width = RESTORATION_PROC_UNIT_SIZE >> ss_x;
    let mut i = 0i32;
    while i < unit_h {
        let v_start = limits.v_start + i;
        let full_stripe_height = RESTORATION_PROC_UNIT_SIZE >> ss_y;
        let runit_offset = RESTORATION_UNIT_OFFSET >> ss_y;
        let tile_stripe = (v_start - tile_rect.top + runit_offset) / full_stripe_height;
        let _frame_stripe = tile_stripe0 + tile_stripe;
        let nominal_stripe_height =
            full_stripe_height - if tile_stripe == 0 { runit_offset } else { 0 };
        let h = nominal_stripe_height.min(limits.v_end - v_start);

        if rui.rtype == RESTORE_SGRPROJ {
            crate::port_sgr::sgrproj_filter_stripe_highbd(
                rui.sgr_ep,
                &rui.sgr_xqd,
                unit_w,
                h,
                procunit_width,
                data,
                data_tl + i as usize * stride,
                stride,
                dst,
                dst_tl + i as usize * dst_stride,
                dst_stride,
                bd,
            );
        } else {
            wiener_filter_stripe_hbd(
                &rui.wiener,
                unit_w,
                h,
                procunit_width,
                data,
                data_tl + i as usize * stride,
                stride,
                dst,
                dst_tl + i as usize * dst_stride,
                dst_stride,
                bd,
            );
        }
        i += h;
    }
}

/// C `svt_aom_highbd_get_sse` (svt_psnr.c:93), the kernel behind
/// `sse_restoration_unit` at `highbd = 1`.
///
/// Reproduces C's decomposition verbatim — 16x16 blocks plus a right strip
/// of `width % 16` and a bottom strip of `height % 16` — INCLUDING the
/// `(uint32_t)` truncation C applies to each partial sum before accumulating
/// into the i64 total. At 10 bits a tall right strip can genuinely exceed
/// 2^32 (15 cols * 384 rows * 1023^2 > 2^32), so the truncation is
/// observable, not cosmetic.
#[allow(clippy::too_many_arguments)]
pub fn sse_region_hbd(
    a: &[u16],
    a_origin: usize,
    a_stride: usize,
    b: &[u16],
    b_origin: usize,
    b_stride: usize,
    width: usize,
    height: usize,
) -> i64 {
    // C `highbd_variance` (svt_psnr.c:78) over a sub-rect, returning i64;
    // the caller truncates to u32.
    let var = |ao: usize, bo: usize, w: usize, h: usize| -> i64 {
        let mut sse = 0i64;
        for i in 0..h {
            for j in 0..w {
                let d = a[ao + i * a_stride + j] as i64 - b[bo + i * b_stride + j] as i64;
                sse += d * d;
            }
        }
        sse
    };
    let dw = width % 16;
    let dh = height % 16;
    let mut total = 0i64;
    if dw > 0 {
        total += var(a_origin + width - dw, b_origin + width - dw, dw, height) as u32 as i64;
    }
    if dh > 0 {
        total += var(
            a_origin + (height - dh) * a_stride,
            b_origin + (height - dh) * b_stride,
            width - dw,
            dh,
        ) as u32 as i64;
    }
    for y in 0..height / 16 {
        for x in 0..width / 16 {
            let ao = a_origin + y * 16 * a_stride + x * 16;
            let bo = b_origin + y * 16 * b_stride + x * 16;
            // `svt_aom_highbd_mse16x16` — always < 2^32 for bd <= 12.
            total += var(ao, bo, 16, 16) as u32 as i64;
        }
    }
    total
}

#[cfg(test)]
mod tests {

    /// The streamed `wiener_convolve_add_src` must equal the materialised form
    /// C writes at every processing-unit shape the restoration filter can ask
    /// for, and at both loop orders. This is the pin for BOTH changes in that
    /// function: the eight-row ring that replaced the heap intermediate, and
    /// the row-major rewrite of the vertical pass.
    /// Every legal tap set, every lane position, every proc-unit width — on
    /// every tier archmage can reach on this host.
    ///
    /// Three things are being pinned, and the third is the one this program
    /// has got wrong fourteen times:
    ///
    /// 1. **The vector arm equals the C-shaped reference.** Not the streamed
    ///    scalar arm — [`wiener_convolve_add_src_materialised`], which is the
    ///    literal transcription of `svt_av1_wiener_convolve_add_src_c`
    ///    (convolve.c:106) and shares no code with the arm under test.
    /// 2. **The tap domain is covered at its CORNERS, not sampled.** The AV1
    ///    tap ranges (restoration.h:141-147) are `t0 in [-5,10]`,
    ///    `t1 in [-23,8]`, `t2 in [-17,46]` with `f[3] = -2*(t0+t1+t2)`; all
    ///    2^3 corner combinations run, plus the midpoint filter
    ///    `WienerInfo::default` and the identity `[0,0,0,128,0,0,0,0]` (the one
    ///    filter that would hide an off-by-one in the centre tap). The
    ///    IMPULSE rounds put a single 255 at each position of the 8x8 window
    ///    in turn, which is what pins the lane-to-tap mapping: a transposed or
    ///    rotated shuffle survives random data far more often than it survives
    ///    a moving impulse.
    /// 3. **More than one tier actually ran.** `permutations_run >= 2` and
    ///    zero archmage warnings. A `_v4` arm that is compiled but never
    ///    entered is this repo's most-repeated defect; a one-arm sweep reports
    ///    PASS for exactly that.
    #[test]
    fn wiener_simd_all_tiers_match_materialised() {
        use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

        let stride = 128usize;
        let rows = 96usize;
        let origin = 8 * stride + 8;

        // Corner + midpoint + identity filters. `sym` builds the 8-tap row the
        // encoder codes: symmetric, `f[7] = 0`, `f[3] = -2*(t0+t1+t2)`.
        let sym = |t0: i16, t1: i16, t2: i16| -> [i16; 8] {
            [t0, t1, t2, -2 * (t0 + t1 + t2), t2, t1, t0, 0]
        };
        let mut filters: alloc::vec::Vec<[i16; 8]> = alloc::vec::Vec::new();
        for &t0 in &[WIENER_FILT_TAP0_MINV as i16, WIENER_FILT_TAP0_MAXV as i16] {
            for &t1 in &[WIENER_FILT_TAP1_MINV as i16, WIENER_FILT_TAP1_MAXV as i16] {
                for &t2 in &[WIENER_FILT_TAP2_MINV as i16, WIENER_FILT_TAP2_MAXV as i16] {
                    filters.push(sym(t0, t1, t2));
                }
            }
        }
        filters.push(WienerInfo::default().vfilter);
        filters.push([0, 0, 0, 128, 0, 0, 0, 0]);

        // Every filter must actually REACH the vector arm — otherwise this
        // whole test is a scalar-vs-scalar tautology.
        for f in &filters {
            assert!(
                wiener_simd_applicable(f, f, 64),
                "a legal Wiener filter {f:?} was refused by the vector arm's \
                 precondition, so the sweep below would not test it"
            );
        }

        let shapes: [(usize, usize); 7] = [
            (16, 8),
            (32, 16),
            (48, 24),
            (64, 64),
            (64, 1),
            (16, 2),
            (8, 56),
        ];

        let mut st = 0x1234_5678u32;
        let mut next = || {
            st ^= st << 13;
            st ^= st >> 17;
            st ^= st << 5;
            st
        };

        let mut perms = 0usize;
        let mut warned = 0usize;
        let mut ran = 0usize;
        let mut tiers: alloc::vec::Vec<&'static str> = alloc::vec::Vec::new();

        // Round kinds: 0 = flat black, 1 = flat white, 2 = random,
        // 3 = column ramp, 4.. = a single 255 impulse walking the window.
        for round in 0..(5 + 64) {
            let mut src = alloc::vec![0u8; stride * rows];
            match round {
                0 => {}
                1 => src.iter_mut().for_each(|v| *v = 255),
                2 => src.iter_mut().for_each(|v| *v = (next() >> 24) as u8),
                3 => {
                    for r in 0..rows {
                        for c in 0..stride {
                            src[r * stride + c] = ((r * 3 + c * 5) & 0xFF) as u8;
                        }
                    }
                }
                4 => src.iter_mut().for_each(|v| *v = 128),
                _ => {
                    // Impulse at window offset (dy, dx) in -3..=4 around the
                    // block origin: exactly the 8x8 support of one output.
                    let k = round - 5;
                    let dy = (k / 8) as isize - 3;
                    let dx = (k % 8) as isize - 3;
                    let idx = origin as isize + dy * stride as isize + dx;
                    src[idx as usize] = 255;
                }
            }

            for (fi, hf) in filters.iter().enumerate() {
                // Pair each h-filter with a different v-filter so an
                // h/v swap cannot pass.
                let vf = &filters[(fi + 3) % filters.len()];
                for &(w, h) in &shapes {
                    let mut want = alloc::vec![0u8; stride * rows];
                    wiener_convolve_add_src_materialised(
                        &src, origin, stride, &mut want, origin, stride, hf, vf, w, h,
                    );
                    let report =
                        for_each_token_permutation(CompileTimePolicy::WarnStderr, |perm| {
                            let tier = wiener_simd_tier();
                            if !tiers.contains(&tier) {
                                tiers.push(tier);
                            }
                            let mut got = alloc::vec![0u8; stride * rows];
                            wiener_convolve_add_src(
                                &src, origin, stride, &mut got, origin, stride, hf, vf, w, h,
                            );
                            for y in 0..h {
                                let a = origin + y * stride;
                                assert_eq!(
                                    &got[a..a + w],
                                    &want[a..a + w],
                                    "wiener tier {perm} != C shape: round {round} \
                                     filter {fi} {w}x{h} row {y}"
                                );
                            }
                        });
                    perms = report.permutations_run;
                    warned = report.warnings.len();
                    ran += 1;
                }
            }
        }

        assert!(ran > 0, "the sweep ran no cells");
        assert_eq!(
            warned, 0,
            "archmage excluded {warned} token(s) from the sweep, so this test \
             covered FEWER tiers than its name claims"
        );
        assert!(
            perms >= 2,
            "the tier sweep ran {perms} permutation(s) -- only the native tier. \
             A one-arm sweep cannot catch a SIMD-vs-scalar divergence, and it \
             is exactly how a `_v4` arm that never executes reports PASS."
        );
        assert!(
            tiers.len() >= 2,
            "the sweep resolved to ONE tier ({tiers:?}); nothing was compared \
             across arms"
        );

        // The point of the whole chunk: on a CPU that HAS AVX-512, the arm the
        // dispatch selects must be the 512-bit one. If this ever starts
        // failing on a Zen 4 / Ice Lake host, the `avx512` feature has been
        // turned off somewhere in the dependency chain and the tier is dead.
        #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
        {
            use archmage::{SimdToken, X64V4Token};
            if X64V4Token::summon().is_some() {
                assert!(
                    tiers.iter().any(|t| t.contains("X64V4Token")),
                    "this CPU reports AVX-512 F/BW/CD/DQ/VL, but the Wiener \
                     dispatch never resolved to the _v4 arm -- tiers seen: \
                     {tiers:?}"
                );
            }
        }
        // Same statement from the other side: on a host WITHOUT AVX-512 (most
        // CI runners) the _v4 arm is compiled and simply never entered, which
        // is correct and is what makes the default-on `avx512` feature safe.
        extern crate std;
        std::eprintln!("wiener vector tiers exercised: {tiers:?}");
    }

    /// The vector arm's precondition must not quietly exclude the shapes the
    /// encoder actually asks for. `wiener_filter_stripe` (restoration.c:399)
    /// calls with `w = procunit_width.min((stripe_width - j + 15) & !15)`, so
    /// luma sees 16/32/48/64 and chroma 16/32; the filters are always
    /// symmetric with `f[7] = 0`.
    #[test]
    fn wiener_simd_arm_covers_every_encoder_call_shape() {
        let f = WienerInfo::default();
        for w in [16usize, 32, 48, 64] {
            assert!(
                wiener_simd_applicable(&f.hfilter, &f.vfilter, w),
                "encoder proc-unit width {w} falls off the vector arm"
            );
        }
        // And it must REFUSE what it cannot compute exactly.
        assert!(
            !wiener_simd_applicable(&f.hfilter, &f.vfilter, 65),
            "a width past the ring must fall back, not overrun it"
        );
        let asym = [1i16, 2, 3, -12, 3, 2, 9, 0];
        assert!(
            !wiener_simd_applicable(&f.hfilter, &asym, 64),
            "the vertical symmetric fold is only valid on a symmetric filter"
        );
        let tap7 = [1i16, 2, 3, -12, 3, 2, 1, 5];
        assert!(
            !wiener_simd_applicable(&f.hfilter, &tap7, 64),
            "a non-zero tap 7 is dropped by the fold and must fall back"
        );
    }

    #[test]
    fn wiener_streaming_matches_materialised() {
        let stride = 256usize;
        let mut st = 0x9E37_79B9u32;
        let mut next = || {
            st ^= st << 13;
            st ^= st >> 17;
            st ^= st << 5;
            (st >> 21) as u8
        };
        let src: alloc::vec::Vec<u8> = (0..stride * 256).map(|_| next()).collect();
        // Origin far enough in that the 3-above / 3-left margins are in bounds.
        let origin = 8 * stride + 8;
        // Real Wiener taps sum to 128 and are symmetric; a few shapes plus the
        // identity filter, which is the one that would hide an off-by-one.
        let filters: [[i16; 8]; 3] = [
            [0, 0, 0, 128, 0, 0, 0, 0],
            [3, -7, 15, 104, 15, -7, 3, 0],
            [-5, 12, -21, 156, -21, 12, -5, 0],
        ];
        for hf in &filters {
            for vf in &filters {
                for &(w, h) in &[
                    (16usize, 8usize),
                    (32, 16),
                    (48, 24),
                    (64, 64),
                    (64, 1),
                    (16, 2),
                    (8, 56),
                ] {
                    let mut a = alloc::vec![0u8; stride * 256];
                    let mut b = alloc::vec![0u8; stride * 256];
                    wiener_convolve_add_src(
                        &src, origin, stride, &mut a, origin, stride, hf, vf, w, h,
                    );
                    wiener_convolve_add_src_materialised(
                        &src, origin, stride, &mut b, origin, stride, hf, vf, w, h,
                    );
                    assert_eq!(a, b, "wiener {w}x{h} hf={hf:?} vf={vf:?}");
                }
            }
        }
    }
    use super::*;

    /// The default WienerInfo must match C set_default_wiener: taps sum with
    /// the implicit +128 center to 128.
    #[test]
    fn default_wiener_taps() {
        let wi = WienerInfo::default();
        assert_eq!(wi.vfilter, [3, -7, 15, -2 * (3 - 7 + 15), 15, -7, 3, 0]);
        let sum: i32 = wi.vfilter.iter().map(|&t| t as i32).sum::<i32>() + 128;
        assert_eq!(sum, 128);
    }

    /// Identity filter (all zero side taps): output == centre input (the
    /// add-src rounding carries the pixel through both passes exactly).
    #[test]
    fn identity_filter_passthrough() {
        let w = 16usize;
        let h = 12usize;
        let b = 4usize;
        let stride = w + 2 * b;
        let mut src = alloc::vec![0u8; stride * (h + 2 * b)];
        let origin = b * stride + b;
        for y in 0..h {
            for x in 0..w {
                src[origin + y * stride + x] = ((x * 13 + y * 7) % 251) as u8;
            }
        }
        extend_frame(&mut src, origin, w, h, stride, 4, 3);
        let zero = WienerInfo {
            vfilter: [0, 0, 0, 0, 0, 0, 0, 0],
            hfilter: [0, 0, 0, 0, 0, 0, 0, 0],
        };
        let mut dst = alloc::vec![0u8; stride * (h + 2 * b)];
        wiener_convolve_add_src(
            &src,
            origin,
            stride,
            &mut dst,
            origin,
            stride,
            &zero.hfilter,
            &zero.vfilter,
            w,
            h,
        );
        for y in 0..h {
            for x in 0..w {
                assert_eq!(dst[origin + y * stride + x], src[origin + y * stride + x]);
            }
        }
    }

    #[test]
    fn count_units_matches_c_rounding() {
        assert_eq!(count_units_in_tile(256, 64), 1);
        assert_eq!(count_units_in_tile(256, 128), 1);
        assert_eq!(count_units_in_tile(256, 256), 1);
        assert_eq!(count_units_in_tile(256, 384), 2); // (384+128)/256 = 2
        assert_eq!(count_units_in_tile(256, 383), 1); // (383+128)/256 = 1
        assert_eq!(count_units_in_tile(64, 32), 1);
    }
}
