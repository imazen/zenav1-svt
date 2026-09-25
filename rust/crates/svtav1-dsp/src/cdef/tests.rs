use super::*;

use alloc::vec;
use archmage::testing::{CompileTimePolicy, TokenPermutation, for_each_token_permutation};

/// Sweep EVERY dispatch arm and fail if the sweep degenerated to the native
/// tier — the silent-coverage hazard `rust/CLAUDE.md` documents: a discarded
/// `PermutationReport` turns an all-tiers test into a one-tier test and it
/// still reads green.
fn for_each_tier(label: &str, f: impl FnMut(&TokenPermutation)) {
    let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, f);
    assert!(
        report.warnings.is_empty(),
        "{label}: archmage excluded {} token(s): {:?}",
        report.warnings.len(),
        report.warnings
    );
    assert!(
        report.permutations_run >= 2,
        "{label}: the tier sweep ran {} permutation(s) -- only the native \
             tier, which cannot catch a SIMD-vs-scalar divergence.",
        report.permutations_run
    );
}

/// The i16 accumulator in [`cdef_rows_dir_v3`] is exact only while
/// `|sum|` stays inside `i16` with room for the `+ 8` rounding. This
/// recomputes the worst case from the ACTUAL tap tables and the legal
/// 8-bit strength ranges (primary `0..=CDEF_PRI_STRENGTHS-1` after
/// `adjust_strength`, which never raises a strength; secondary
/// `sec + (sec == 3)` over `sec in 0..=3`, i.e. `{0,1,2,4}`), so a future
/// change to either table trips this test rather than the bitstream.
#[test]
fn i16_sum_accumulator_cannot_overflow_on_the_legal_strength_domain() {
    let max_pri = CDEF_PRI_STRENGTHS - 1; // 15
    let max_sec = 3 + 1; // sec == 3 signals strength 4
    let mut worst = 0i32;
    for taps in 0..2usize {
        // 2 primary taps of each coefficient, 4 secondary taps of each.
        let s = 2 * CDEF_PRI_TAPS[taps][0] * max_pri
            + 2 * CDEF_PRI_TAPS[taps][1] * max_pri
            + 4 * CDEF_SEC_TAPS[taps][0] * max_sec
            + 4 * CDEF_SEC_TAPS[taps][1] * max_sec;
        worst = worst.max(s);
    }
    assert_eq!(worst, 228, "tap tables or strength ranges changed");
    assert!(
        worst + 8 <= i16::MAX as i32 && -worst > i16::MIN as i32,
        "the i16 sum + 8 rounding can overflow: worst |sum| = {worst}"
    );
}

/// A deterministic xorshift so the buffers are reproducible without a dep.
fn lcg(state: &mut u64) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state >> 32) as u32
}

/// Fill a padded CDEF input buffer with one of the named patterns.
/// `kind` 0..=4 are the extremes (flat 0, flat 255, all-sentinel,
/// 0/255 checkerboard, a ramp across the WHOLE `0..=CDEF_VERY_LARGE`
/// value range so every reachable tap difference sign and magnitude is
/// exercised, not just the `0..=255` an 8-bit plane can hold); 5.. are
/// pseudorandom 8-bit pixels with ~1 in 8 sentinels, which is what the
/// real frame edges look like.
fn fill_inbuf(buf: &mut [u16], kind: usize, seed: u64) {
    let mut st = seed | 1;
    for (i, v) in buf.iter_mut().enumerate() {
        *v = match kind {
            0 => 0,
            1 => 255,
            2 => CDEF_VERY_LARGE,
            3 => {
                if (i + i / CDEF_BSTRIDE).is_multiple_of(2) {
                    0
                } else {
                    255
                }
            }
            4 => ((i as u32 * 4099) % (CDEF_VERY_LARGE as u32 + 1)) as u16,
            _ => {
                let r = lcg(&mut st);
                if r.is_multiple_of(8) {
                    CDEF_VERY_LARGE
                } else {
                    (r % 256) as u16
                }
            }
        };
    }
}

/// The vector `cdef_find_dir` arms against the scalar reference, over both
/// `coeff_shift` domains, both strides the pipeline uses, and — the part
/// the C-parity suite does not reach — the `> 255` BAIL-OUT BOUNDARY that
/// `cdef_dir_partials_v3` / `cdef_dir_partials_neon` guard their i16
/// partials with.
///
/// ANTI-VACUITY: the test counts how many blocks took the vector path and
/// how many fell back, and FAILS if either count is zero. Without that a
/// content set that happens to bail on every block would exercise only the
/// scalar arm and still read green — the shape `WORKING-ON-THIS.md` §5
/// calls "a silent harness and a genuine absence are indistinguishable".
#[test]
fn cdef_find_dir_simd_matches_scalar_including_the_bailout_boundary() {
    for_each_tier(
        "cdef_find_dir_simd_matches_scalar_including_the_bailout_boundary",
        |_| {
            let mut in_domain = 0u64;
            let mut bailed = 0u64;
            let mut checked = 0u64;
            for stride in [8usize, CDEF_BSTRIDE] {
                let mut buf = vec![0u16; stride * 8 + 16];
                for shift in [0i32, 2] {
                    let mut st = 0x2545_F491_4F6C_DD1Du64 ^ (stride as u64);
                    for kind in 0..64usize {
                        for (n, v) in buf.iter_mut().enumerate() {
                            let i = n / stride;
                            let j = n % stride;
                            *v = match kind {
                                // Flats and the two values that sit exactly
                                // ON the `<= 255 after shift` boundary.
                                0 => 0,
                                1 => (255u32 << shift) as u16,
                                2 => ((255u32 << shift) | ((1 << shift) - 1)) as u16,
                                // One pixel over the boundary -> must bail.
                                3 => {
                                    if n == 3 * stride + 4 {
                                        (256u32 << shift) as u16
                                    } else {
                                        128 << shift
                                    }
                                }
                                // The eight directional ramps, so every
                                // `partial[k]` in turn is the winner.
                                4..=11 => {
                                    // SIGNED: the loop walks the WHOLE
                                    // padded buffer, so `j` reaches
                                    // `stride - 1` and `7 + i - j`
                                    // underflows a `usize`. That is
                                    // invisible under `--release` (no
                                    // overflow checks) and panics under
                                    // nextest's debug profile — which is
                                    // exactly how it was caught.
                                    let (i, j) = (i as i64, j as i64);
                                    let t = match kind - 4 {
                                        0 => i + j,
                                        1 => i + j / 2,
                                        2 => i,
                                        3 => 3 + i - j / 2,
                                        4 => 7 + i - j,
                                        5 => 3 - i / 2 + j,
                                        6 => j,
                                        _ => i / 2 + j,
                                    };
                                    (((t * 17).rem_euclid(256)) as u16) << shift
                                }
                                // Over the boundary in every pixel, so
                                // every block bails to the scalar arm.
                                //
                                // Capped at 320 rather than swept over the
                                // whole u16 range for a REASON worth
                                // recording: the shared cost tail
                                // [`cdef_dir_from_partials`] — and C's
                                // `svt_aom_cdef_find_dir_c`, which it is
                                // transcribed from — accumulates in i32,
                                // and `cost[0]` is bounded by roughly
                                // `53,760 * x^2` where `x = (px >> shift)
                                // - 128`. That overflows i32 for
                                // `|x| > ~197`, i.e. a shifted pixel above
                                // ~325. Real callers never get there
                                // (reconstructed pixels are <= 255 after
                                // the bit-depth shift, which is also what
                                // the vector arms' `> 255` bail-out
                                // enforces), and in release both the port
                                // and C simply wrap. A debug build panics,
                                // so a test must not drive the reference
                                // outside the range the reference can
                                // represent.
                                12..=15 => 256 + (lcg(&mut st) % 65) as u16,
                                // 8-bit noise in the shifted domain.
                                _ => {
                                    (((lcg(&mut st) % 256) as u16) << shift)
                                        | ((lcg(&mut st) as u16) & ((1 << shift) - 1))
                                }
                            };
                        }
                        let over = (0..8usize)
                            .any(|i| (0..8usize).any(|j| (buf[i * stride + j] >> shift) > 255));
                        if over {
                            bailed += 1;
                        } else {
                            in_domain += 1;
                        }
                        let want =
                            cdef_dir_from_partials(&cdef_dir_partials_scalar(&buf, stride, shift));
                        let got = cdef_find_dir(&buf, stride, shift);
                        assert_eq!(
                            got, want,
                            "find_dir != scalar: stride={stride} shift={shift} kind={kind}"
                        );
                        checked += 1;
                    }
                }
            }
            assert_eq!(checked, 2 * 2 * 64);
            assert!(
                in_domain > 0 && bailed > 0,
                "vacuous sweep: {in_domain} blocks took the vector path and \
                     {bailed} fell back — both must be non-zero for this test to \
                     have covered what its name claims"
            );
        },
    );
}

/// EVERY legal knob combination the encoder can hand the dst8 filter, on
/// nine input patterns each, compared to [`cdef_filter_block_core`] byte
/// for byte through the public `incant!` dispatcher — so every tier the
/// host offers is swept (the `for_each_tier` report is CONSUMED).
///
/// The knob space is exhaustive: all four `bsize`s, all 8 directions, all
/// 16 primary strengths, all 4 SIGNALLED secondary strengths mapped
/// through `sec + (sec == 3)`, dampings 2..=6 (luma `3 + (qindex >> 6)`
/// is 3..=6 and chroma subtracts 1), and the subsampling factors the
/// search actually uses (`sub_y = min(cfg, 4)`, `sub_uv = 1`) plus 2.
/// The pixel space is NOT exhaustive — it cannot be, 12 taps of u16 — but
/// the patterns include the flats, the all-sentinel case, the maximum-
/// contrast checkerboard and a ramp over the entire `0..=CDEF_VERY_LARGE`
/// range, so every `constrain` branch and every sentinel path is reached.
#[test]
fn cdef_filter_block_simd_matches_scalar_over_the_legal_knob_domain() {
    for_each_tier(
        "cdef_filter_block_simd_matches_scalar_over_the_legal_knob_domain",
        |_| {
            let ioff = CDEF_VBORDER * CDEF_BSTRIDE + CDEF_HBORDER;
            let mut inb = vec![0u16; CDEF_INBUF_SIZE];
            let dstride = 16usize;
            let mut got = vec![0u8; dstride * 16];
            let mut want = vec![0u8; dstride * 16];
            let mut checked = 0u64;
            for kind in 0..9usize {
                fill_inbuf(&mut inb, kind, 0x9E37_79B9_7F4A_7C15 ^ (kind as u64));
                for &bsize in &[BLOCK_4X4, BLOCK_4X8, BLOCK_8X4, BLOCK_8X8] {
                    for dir in 0..8i32 {
                        for pri in 0..CDEF_PRI_STRENGTHS {
                            for sec_ix in 0..CDEF_SEC_STRENGTHS {
                                let sec = sec_ix + i32::from(sec_ix == 3);
                                for damping in 2..=6i32 {
                                    for &sub in &[1usize, 2, 4] {
                                        got.fill(0xAA);
                                        want.fill(0xAA);
                                        cdef_filter_block(
                                            &mut got, 0, dstride, &inb, ioff, pri, sec, dir,
                                            damping, damping, bsize, 0, sub,
                                        );
                                        cdef_filter_block_core(
                                            &mut want, 0, dstride, &inb, ioff, pri, sec, dir,
                                            damping, damping, bsize, 0, sub,
                                        );
                                        assert_eq!(
                                            got, want,
                                            "kind={kind} bsize={bsize} dir={dir} \
                                                 pri={pri} sec={sec} damping={damping} \
                                                 sub={sub}"
                                        );
                                        checked += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // 9 patterns x 4 bsizes x 8 dirs x 16 pri x 4 sec x 5 damping x 3 sub
            assert_eq!(checked, 9 * 4 * 8 * 16 * 4 * 5 * 3);
        },
    );
}

/// `cdef_dist_packed` under every dispatch tier must equal the scalar
/// core — over both block dims, every legal subsampling (including 3,
/// which does not divide 8 and exercises the staged-tail fold), and an
/// edge-clipped block list.
#[test]
fn cdef_dist_packed_all_tiers_match_scalar() {
    for_each_tier("cdef_dist_packed_all_tiers_match_scalar", |_| {
        let pw = 96usize;
        let ph = 96usize;
        let mut plane = vec![0u8; pw * ph];
        let mut packed = vec![0u8; 64 * 64];
        for (i, b) in plane.iter_mut().enumerate() {
            *b = ((i * 37 + (i >> 6) * 91) & 0xFF) as u8;
        }
        for (i, b) in packed.iter_mut().enumerate() {
            *b = ((i * 53 + (i >> 5) * 17) & 0xFF) as u8;
        }
        let blocks: alloc::vec::Vec<(usize, usize)> = (0..7)
            .flat_map(|by| (0..7).map(move |bx| (by, bx)))
            .collect();
        for &dim in &[8usize, 4] {
            let nblk = blocks.len().min(packed.len() / (dim * dim));
            let bl = &blocks[..nblk];
            for &sub in &[1usize, 2, 3, 4] {
                let got =
                    cdef_dist_packed(&plane, 0, pw, &packed[..nblk * dim * dim], bl, dim, sub);
                let want =
                    cdef_dist_packed_core(&plane, 0, pw, &packed[..nblk * dim * dim], bl, dim, sub);
                assert_eq!(got, want, "dim={dim} sub={sub}");
            }
        }
    });
}

/// Spec 7.15.3 Cdef_Directions cross-check: the padded table's live rows
/// (index 2..10) decoded back to (dy, dx) must equal the spec table.
#[test]
fn direction_table_matches_spec() {
    const SPEC: [[[i32; 2]; 2]; 8] = [
        [[-1, 1], [-2, 2]],
        [[0, 1], [-1, 2]],
        [[0, 1], [0, 2]],
        [[0, 1], [1, 2]],
        [[1, 1], [2, 2]],
        [[1, 0], [2, 1]],
        [[1, 0], [2, 0]],
        [[1, 0], [2, -1]],
    ];
    let s = CDEF_BSTRIDE as i32;
    for dir in 0..8 {
        for k in 0..2 {
            let off = cdef_direction(dir, k);
            // decode: dy = round-to-nearest row (offsets have |dx| <= 2)
            let dy = if off >= 0 {
                (off + s / 2) / s
            } else {
                -((-off + s / 2) / s)
            };
            let dx = off - dy * s;
            assert_eq!([dy, dx], SPEC[dir as usize][k], "dir {dir} k {k}");
        }
    }
    // padded ends replicate dir 6,7 and 0,1
    assert_eq!(cdef_direction(-2, 0), cdef_direction(6, 0));
    assert_eq!(cdef_direction(-1, 1), cdef_direction(7, 1));
    assert_eq!(cdef_direction(8, 0), cdef_direction(0, 0));
    assert_eq!(cdef_direction(9, 1), cdef_direction(1, 1));
}

/// A flat block has no direction energy: var must be 0 and filtering at
/// any strength must be the identity.
#[test]
fn flat_block_identity() {
    let mut inb = alloc::vec![CDEF_VERY_LARGE; CDEF_INBUF_SIZE];
    let ioff = CDEF_VBORDER * CDEF_BSTRIDE + CDEF_HBORDER;
    for r in 0..8 {
        for c in 0..8 {
            inb[ioff + r * CDEF_BSTRIDE + c] = 77;
        }
    }
    let (_dir, var) = cdef_find_dir(&inb[ioff..], CDEF_BSTRIDE, 0);
    assert_eq!(var, 0);
    let mut dst = [0u8; 64];
    cdef_filter_block(&mut dst, 0, 8, &inb, ioff, 15, 4, 3, 6, 6, BLOCK_8X8, 0, 1);
    assert!(dst.iter().all(|&v| v == 77));
}

/// constrain() reproduces the C damping shape at hand-checked points.
#[test]
fn constrain_c_values() {
    // threshold 0 -> 0 regardless
    assert_eq!(constrain(1000, 0, 6), 0);
    // shift = max(0, 4 - msb(4)=2) = 2: c(5,4,4) = min(5, 4 - (5>>2)) = 3
    assert_eq!(constrain(5, 4, 4), 3);
    assert_eq!(constrain(-5, 4, 4), -3);
    // sentinel-sized diff is fully damped to 0
    assert_eq!(constrain(0x7f7f - 128, 15, 6), 0);
    assert_eq!(constrain(0x4000 - 128, 15, 6), 0);
    // large threshold, small diff: passes through
    assert_eq!(constrain(2, 15, 3), 2);
}

/// adjust_strength C anchor points.
#[test]
fn adjust_strength_c_values() {
    assert_eq!(adjust_strength(12, 0), 0);
    // var=63: var>>6 = 0 -> i=0 -> (12*4+8)>>4 = 3
    assert_eq!(adjust_strength(12, 63), 3);
    // var=1<<18: i = min(msb(1<<12)=12, 12) -> (12*16+8)>>4 = 12
    assert_eq!(adjust_strength(12, 1 << 18), 12);
}
