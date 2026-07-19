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
//!   decoder-authoritative stripe walk (libaom av1/common/restoration.c is
//!   the same code).
//! - Boundary capture: `svt_aom_save_deblock_boundary_lines` /
//!   `svt_aom_save_cdef_boundary_lines` / `svt_aom_save_tile_row_boundary_lines`
//!   (restoration.c:1507-1662) — the two-pass (post-deblock, post-CDEF)
//!   line-buffer scheme, single-tile form.
//!
//! Every function here is differentially fuzzed against the C archive in
//! `tests/c_parity_wiener.rs`.

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
    let win2 = wiener_win * wiener_win;
    let halfwin = (wiener_win >> 1) as i32;
    assert!(m.len() >= win2 && h.len() >= win2 * win2);
    let avg = find_average(dgd, dgd_origin, dgd_stride, h_start, h_end, v_start, v_end) as i16;

    m[..win2].fill(0);
    h[..win2 * win2].fill(0);
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
            for k in 0..win2 {
                m[k] += (y[k] as i32 * x as i32) as i64;
                for l in k..win2 {
                    h[k * win2 + l] += (y[k] as i32 * y[l] as i32) as i64;
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

/// WIENER_TAP_SCALE_FACTOR (restoration_pick.c:31).
const WIENER_TAP_SCALE_FACTOR: i64 = 1 << 16;

/// C `wrap_index` (restoration_pick.c:745).
#[inline]
fn wrap_index(i: usize, wiener_win: usize) -> usize {
    let halfwin1 = (wiener_win >> 1) + 1;
    if i >= halfwin1 {
        wiener_win - 1 - i
    } else {
        i
    }
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
pub fn wiener_decompose_sep_sym(wiener_win: usize, m: &[i64], h: &[i64], a: &mut [i32], b: &mut [i32]) {
    const INIT_FILT: [i32; WIENER_WIN] = [
        WIENER_FILT_TAP0_MIDV,
        WIENER_FILT_TAP1_MIDV,
        WIENER_FILT_TAP2_MIDV,
        WIENER_FILT_STEP - 2 * (WIENER_FILT_TAP0_MIDV + WIENER_FILT_TAP1_MIDV + WIENER_FILT_TAP2_MIDV),
        WIENER_FILT_TAP2_MIDV,
        WIENER_FILT_TAP1_MIDV,
        WIENER_FILT_TAP0_MIDV,
    ];
    let plane_off = (WIENER_WIN - wiener_win) >> 1;
    for i in 0..wiener_win {
        let v = (WIENER_TAP_SCALE_FACTOR / WIENER_FILT_STEP as i64) as i32
            * INIT_FILT[i + plane_off];
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
pub fn compute_score(wiener_win: usize, m: &[i64], h: &[i64], vfilt: &[i16; 8], hfilt: &[i16; 8]) -> i64 {
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

/// C `RestorationStripeBoundaries` (restoration.h:217), 8-bit only. Buffer
/// column `i` corresponds to plane column `i - RESTORATION_EXTRA_HORZ`;
/// row `RESTORATION_CTX_VERT * frame_stripe + j` holds the j-th saved line
/// of that stripe's boundary.
#[derive(Clone, Debug, Default)]
pub struct StripeBoundaries {
    pub above: alloc::vec::Vec<u8>,
    pub below: alloc::vec::Vec<u8>,
    pub stride: usize,
}

/// C `get_stripe_boundary_info` (restoration.c:216).
fn get_stripe_boundary_info(
    limits: &TileLimits,
    tile_rect: &PixelRect,
    ss_y: i32,
) -> (bool, bool) {
    let mut copy_above = true;
    let mut copy_below = true;

    let full_stripe_height = RESTORATION_PROC_UNIT_SIZE >> ss_y;
    let runit_offset = RESTORATION_UNIT_OFFSET >> ss_y;

    let first_stripe_in_tile = limits.v_start == tile_rect.top;
    let this_stripe_height = full_stripe_height - if first_stripe_in_tile { runit_offset } else { 0 };
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
/// 8-bit, boundary rows only (the cdef/lr column buffers are unused in the
/// single-tile path).
struct LineBuffers {
    above: [[u8; 400]; RESTORATION_BORDER as usize],
    below: [[u8; 400]; RESTORATION_BORDER as usize],
}

/// C `setup_processing_stripe_boundary` (restoration.c:249), opt=0, 8-bit.
#[allow(clippy::too_many_arguments)]
fn setup_processing_stripe_boundary(
    limits: &TileLimits,
    rsb: &StripeBoundaries,
    rsb_row: i32,
    h: i32,
    data: &mut [u8],
    data_origin: usize,
    data_stride: usize,
    rlbs: &mut LineBuffers,
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

/// C `restore_processing_stripe_boundary` (restoration.c:347), opt=0, 8-bit.
#[allow(clippy::too_many_arguments)]
fn restore_processing_stripe_boundary(
    limits: &TileLimits,
    rlbs: &LineBuffers,
    h: i32,
    data: &mut [u8],
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

/// C `svt_av1_loop_restoration_filter_unit` (restoration.c:1040), 8-bit,
/// wiener/none only (sgrproj is never searched or signaled at the ported
/// presets — sg_filter_lvl = 0).
///
/// `data`/`dst` are padded planes; `*_origin` indexes plane (0,0). `data` is
/// temporarily modified around stripe boundaries when `need_boundaries` is
/// set (decoder-exact application); the search path passes false
/// (`use_boundaries_in_rest_search = 0`, enc_handle.c:4483).
#[allow(clippy::too_many_arguments)]
pub fn loop_restoration_filter_unit(
    need_boundaries: bool,
    limits: &TileLimits,
    rtype: u8,
    wiener: &WienerInfo,
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
    let unit_h = limits.v_end - limits.v_start;
    let unit_w = limits.h_end - limits.h_start;
    let data_tl = data_origin + limits.v_start as usize * stride + limits.h_start as usize;
    let dst_tl = dst_origin + limits.v_start as usize * dst_stride + limits.h_start as usize;

    if rtype == RESTORE_NONE {
        for i in 0..unit_h as usize {
            let s = data_tl + i * stride;
            let d = dst_tl + i * dst_stride;
            let (a, b) = (s..s + unit_w as usize, d..d + unit_w as usize);
            dst[b].copy_from_slice(&data[a]);
        }
        return;
    }
    debug_assert_eq!(rtype, RESTORE_WIENER);

    let procunit_width = RESTORATION_PROC_UNIT_SIZE >> ss_x;
    let mut rlbs = LineBuffers {
        above: [[0; 400]; RESTORATION_BORDER as usize],
        below: [[0; 400]; RESTORATION_BORDER as usize],
    };

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
                &remaining, rsb, rsb_row, h, data, data_origin, stride, &mut rlbs, copy_above,
                copy_below,
            );
        }
        wiener_filter_stripe(
            wiener,
            unit_w,
            h,
            procunit_width,
            data,
            data_tl + i as usize * stride,
            stride,
            dst,
            dst_tl + i as usize * dst_stride,
            dst_stride,
        );
        if need_boundaries {
            restore_processing_stripe_boundary(
                &remaining, &rlbs, h, data, data_origin, stride, copy_above, copy_below,
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
        let h = if remaining_h < ext_size { remaining_h } else { unit_size };

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
            let w = if remaining_w < ext_size { remaining_w } else { unit_size };
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

/// C `extend_lines` (restoration.c:1492), 8-bit.
fn extend_lines(buf: &mut [u8], start: usize, width: usize, height: usize, stride: usize, extend: usize) {
    for i in 0..height {
        let row = start + i * stride;
        let left = buf[row];
        let right = buf[row + width - 1];
        buf[row - extend..row].fill(left);
        buf[row + width..row + width + extend].fill(right);
    }
}

/// C `svt_aom_save_deblock_boundary_lines` (restoration.c:1507), no superres.
#[allow(clippy::too_many_arguments)]
fn save_deblock_boundary_lines(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    src_width: i32,
    src_height: i32,
    row: i32,
    stripe: i32,
    is_above: bool,
    boundaries: &mut StripeBoundaries,
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

/// C `svt_aom_save_cdef_boundary_lines` (restoration.c:1561), no superres.
#[allow(clippy::too_many_arguments)]
fn save_cdef_boundary_lines(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    src_width: i32,
    row: i32,
    stripe: i32,
    is_above: bool,
    boundaries: &mut StripeBoundaries,
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
#[allow(clippy::too_many_arguments)]
pub fn save_tile_row_boundary_lines(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    src_width: i32,
    src_height: i32,
    ss_y: i32,
    after_cdef: bool,
    boundaries: &mut StripeBoundaries,
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
                    src, src_origin, src_stride, src_width, y0, frame_stripe, true, boundaries,
                );
            }
            if !use_deblock_below {
                save_cdef_boundary_lines(
                    src, src_origin, src_stride, src_width, y1 - 1, frame_stripe, false, boundaries,
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
    let ext_h = RESTORATION_UNIT_OFFSET + frame_height;
    let num_stripes = (ext_h + 63) / 64;
    let plane_w = ((frame_width + ss_x) >> ss_x) + 2 * RESTORATION_EXTRA_HORZ;
    // ALIGN_POWER_OF_TWO(plane_w, 5)
    let stride = ((plane_w + 31) & !31) as usize;
    let size = num_stripes as usize * stride * RESTORATION_CTX_VERT as usize;
    StripeBoundaries {
        above: alloc::vec![0u8; size],
        below: alloc::vec![0u8; size],
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
            for k in 0..win2 {
                m[k] += y[k] as i64 * x as i64;
                for l in k..win2 {
                    h[k * win2 + l] += y[k] as i64 * y[l] as i64;
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
    rtype: u8,
    wiener: &WienerInfo,
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

    if rtype == RESTORE_NONE {
        for i in 0..unit_h as usize {
            let s = data_tl + i * stride;
            let d = dst_tl + i * dst_stride;
            dst[d..d + unit_w as usize].copy_from_slice(&data[s..s + unit_w as usize]);
        }
        return;
    }
    debug_assert_eq!(rtype, RESTORE_WIENER);

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

        wiener_filter_stripe_hbd(
            wiener,
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
            &src, origin, stride, &mut dst, origin, stride, &zero.hfilter, &zero.vfilter, w, h,
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
