use super::*;

#[cfg(target_arch = "aarch64")]
#[derive(Clone, Copy)]
pub(super) struct CsV(int16x8_t, int16x8_t);

#[cfg(target_arch = "aarch64")]
#[derive(Clone, Copy)]
pub(super) struct CsA(int32x4_t, int32x4_t);

#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn cs_load_neon(_t: NeonToken, a: &[i16; 16]) -> CsV {
    let lo: &[i16; 8] = a[..8].try_into().unwrap();
    let hi: &[i16; 8] = a[8..].try_into().unwrap();
    CsV(vld1q_s16(lo), vld1q_s16(hi))
}

/// Lane mask: lanes `< n` all-ones, the rest zero (`n <= 16`).
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn cs_mask_neon(_t: NeonToken, n: usize) -> CsV {
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
pub(super) fn cs_and_neon(_t: NeonToken, a: CsV, b: CsV) -> CsV {
    CsV(vandq_s16(a.0, b.0), vandq_s16(a.1, b.1))
}

#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn cs_zero_neon(_t: NeonToken) -> CsA {
    CsA(vdupq_n_s32(0), vdupq_n_s32(0))
}

#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn cs_madd_neon(_t: NeonToken, acc: CsA, a: CsV, b: CsV) -> CsA {
    let a0 = vmlal_s16(acc.0, vget_low_s16(a.0), vget_low_s16(b.0));
    let a0 = vmlal_high_s16(a0, a.0, b.0);
    let a1 = vmlal_s16(acc.1, vget_low_s16(a.1), vget_low_s16(b.1));
    let a1 = vmlal_high_s16(a1, a.1, b.1);
    CsA(a0, a1)
}

#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn cs_msub_neon(_t: NeonToken, acc: CsA, a: CsV, b: CsV) -> CsA {
    let a0 = vmlsl_s16(acc.0, vget_low_s16(a.0), vget_low_s16(b.0));
    let a0 = vmlsl_high_s16(a0, a.0, b.0);
    let a1 = vmlsl_s16(acc.1, vget_low_s16(a.1), vget_low_s16(b.1));
    let a1 = vmlsl_high_s16(a1, a.1, b.1);
    CsA(a0, a1)
}

/// Horizontal sum of the eight `i32` lanes, widening as it adds.
#[cfg(target_arch = "aarch64")]
#[rite]
pub(super) fn cs_reduce_neon(_t: NeonToken, acc: CsA) -> i64 {
    vaddlvq_s32(acc.0) + vaddlvq_s32(acc.1)
}

/// C's `find_average_neon`: the region's mean pixel, 16 bytes at a time
/// through the pairwise-widening adds (`u8 -> u16 -> u32 -> u64`). Exact —
/// the same `u64` sum and truncating divide as [`find_average`].
#[cfg(target_arch = "aarch64")]
#[rite]
#[allow(clippy::too_many_arguments)]
pub(super) fn cs_find_average_neon(
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
pub(super) const CS_MAX_DIM: usize = 32_000;

/// Which regions the C-shape arms take (the rest go to the scalar core):
/// C's two window sizes only (`compute_stats_win{5,7}`), non-empty, and
/// inside [`CS_MAX_DIM`] on both axes.
pub(super) fn cs_accepts(wiener_win: usize, width: usize, height: usize) -> bool {
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
pub(super) struct CsGeom {
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) hw: usize,
    pub(super) dw: usize,
    pub(super) dh: usize,
    pub(super) ds: usize,
    pub(super) ss: usize,
    pub(super) ts: usize,
}

impl CsGeom {
    pub(super) fn new(wiener_win: usize, width: usize, height: usize) -> Self {
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
    pub(super) fn d_len(&self) -> usize {
        self.dh * self.ds
    }
    pub(super) fn s_len(&self) -> usize {
        self.height * self.ss
    }
    pub(super) fn t_len(&self) -> usize {
        4 * self.hw * self.ts
    }
}

/// C's `sub_avg_block_avx2` (`pickrst_avx2.c:202`) / NEON `compute_sub_avg`,
/// plus the transposed edge strips. Plain scalar code — LLVM vectorises the
/// subtract; it is one pass over the region against the kernel's ~134
/// multiply-accumulates per pixel.
#[allow(clippy::too_many_arguments)]
pub(super) fn cs_prepare(
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
pub(super) fn cs_mirror(win2: usize, h: &mut [i64]) {
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
pub(super) fn cs_chunks(p: &[i16], n: usize) -> &[[i16; 16]] {
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
pub(super) fn compute_stats_impl_neon(
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
pub(super) struct StatsScratch {
    pub(super) buf: alloc::vec::Vec<i16>,
    pub(super) dlen: usize,
    pub(super) slen: usize,
    #[cfg(feature = "std")]
    pub(super) pooled: bool,
}

#[cfg(feature = "std")]
std::thread_local! {
    static STATS_SCRATCH: core::cell::RefCell<alloc::vec::Vec<i16>> =
        const { core::cell::RefCell::new(alloc::vec::Vec::new()) };
}

impl StatsScratch {
    pub(super) fn take3(dlen: usize, slen: usize, tlen: usize) -> Self {
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

    pub(super) fn split3(&mut self) -> (&mut [i16], &mut [i16], &mut [i16]) {
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
pub(super) fn compute_stats_scalar_core(
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
pub(super) fn compute_stats_impl_v3(
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
pub(super) const WIENER_TAP_SCALE_FACTOR: i64 = 1 << 16;

/// C `wrap_index` (restoration_pick.c:745).
#[inline]
pub(super) fn wrap_index(i: usize, wiener_win: usize) -> usize {
    let halfwin1 = (wiener_win >> 1) + 1;
    if i >= halfwin1 { wiener_win - 1 - i } else { i }
}

/// C `linsolve_wiener` (restoration_pick.c:752). Returns false when singular.
pub(super) fn linsolve_wiener(
    n: usize,
    a: &mut [i64],
    stride: usize,
    b: &mut [i64],
    x: &mut [i32],
) -> bool {
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
pub(super) fn update_a_sep_sym(wiener_win: usize, m: &[i64], h: &[i64], a: &mut [i32], b: &[i32]) {
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
pub(super) fn update_b_sep_sym(wiener_win: usize, m: &[i64], h: &[i64], a: &[i32], b: &mut [i32]) {
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
pub(super) fn normalize_and_solve(
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
