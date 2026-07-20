//! Differential parity: CDEF kernels vs the C reference
//! (`svt_aom_cdef_find_dir_c` / `svt_aom_cdef_find_dir_8bit_c` /
//! `svt_cdef_filter_block_c` / `svt_cdef_filter_block_8bit_c`).
//!
//! The filter kernel is what the DECODER effectively runs per 8x8 (libaom
//! cdef_filter_block_internal is the same math — see svtav1_dsp::cdef module
//! docs); one bit of divergence breaks recon parity frame-wide. Coverage:
//! every (packed strength 0..=63, damping, direction, bsize) combination the
//! frame-header syntax can signal, over bordered buffers replicating the
//! av1_cdef_frame / svt_av1_cdef_frame unavailable-pixel conventions
//! (CDEF_VERY_LARGE fills at frame boundaries), plus random-sentinel and
//! raw-u16 torture patterns.

use svtav1_cref as cref;
use svtav1_dsp::cdef;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn byte(&mut self) -> u8 {
        (self.next() >> 32) as u8
    }
    fn range(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// 8x8 content classes for the direction search.
fn fill_dir_block(content: u32, rng: &mut Rng, shift: i32, img: &mut [u16; 64]) {
    let max = (255u32 << shift) as u16;
    match content {
        // pure random full pixel range
        0 => {
            for v in img.iter_mut() {
                *v = (rng.range(max as u64 + 1)) as u16;
            }
        }
        // directional gradient at a random angle
        1 => {
            let dx = rng.range(9) as i32 - 4;
            let dy = rng.range(9) as i32 - 4;
            let base = rng.range(128) as i32;
            for r in 0..8i32 {
                for c in 0..8i32 {
                    let v = (base + r * dy * 13 + c * dx * 13).clamp(0, 255) << shift;
                    img[(r * 8 + c) as usize] = v as u16;
                }
            }
        }
        // hard step edge, random orientation/position
        2 => {
            let pos = 1 + rng.range(6) as i32;
            let vertical = rng.range(2) == 0;
            let (lo, hi) = ((rng.byte() >> 2) as u16, 200 + (rng.byte() % 55) as u16);
            for r in 0..8i32 {
                for c in 0..8i32 {
                    let sel = if vertical { c } else { r };
                    img[(r * 8 + c) as usize] = (if sel < pos { lo } else { hi }) << shift;
                }
            }
        }
        // near-flat with tiny noise (var ~ 0 paths)
        _ => {
            let base = rng.byte() as u16;
            for v in img.iter_mut() {
                *v = (base.saturating_add((rng.next() % 3) as u16)) << shift;
            }
        }
    }
}

/// find_dir must agree on (dir, var) for 8-bit and shifted (HBD-domain)
/// content across content classes.
#[test]
fn find_dir_matches_c() {
    let mut rng = Rng(0x1CDEF_D1F);
    for shift in [0i32, 2] {
        for content in 0..4u32 {
            for _ in 0..800 {
                let mut img = [0u16; 64];
                fill_dir_block(content, &mut rng, shift, &mut img);
                let (rd, rv) = cdef::cdef_find_dir(&img, 8, shift);
                let (cd, cv) = cref::cdef_find_dir(&img, 8, shift);
                assert_eq!(
                    (rd, rv),
                    (cd, cv),
                    "find_dir diverges (shift {shift} content {content}): img {img:?}"
                );
            }
        }
    }
    // Non-8 stride (the frame pass searches at CDEF_BSTRIDE).
    let stride = cdef::CDEF_BSTRIDE;
    let mut buf = vec![0u16; stride * 8 + 8];
    for _ in 0..300 {
        for v in buf.iter_mut() {
            *v = rng.byte() as u16;
        }
        let (rd, rv) = cdef::cdef_find_dir(&buf, stride, 0);
        let (cd, cv) = cref::cdef_find_dir(&buf, stride, 0);
        assert_eq!((rd, rv), (cd, cv), "find_dir diverges at stride {stride}");
    }
}

/// The 8-bit wrapper (widen + delegate) must match C's.
#[test]
fn find_dir_8bit_matches_c() {
    let mut rng = Rng(0x8B17_D1F);
    for stride in [8usize, 32, 144] {
        let mut buf = vec![0u8; stride * 8 + 8];
        for _ in 0..600 {
            for v in buf.iter_mut() {
                *v = rng.byte();
            }
            let (rd, rv) = cdef::cdef_find_dir_8bit(&buf, stride, 0);
            let (cd, cv) = cref::cdef_find_dir_8bit(&buf, stride, 0);
            assert_eq!(
                (rd, rv),
                (cd, cv),
                "find_dir_8bit diverges, stride {stride}"
            );
        }
    }
}

const IOFF: usize = cdef::CDEF_VBORDER * cdef::CDEF_BSTRIDE + cdef::CDEF_HBORDER;

/// Border sentinel patterns replicating what cdef_prepare_fb produces for a
/// frame-corner / frame-edge / interior filter block, plus adversarial
/// scatter (the kernel is geometry-blind: any sentinel layout is legal).
fn apply_borders(pattern: u32, rng: &mut Rng, buf: &mut [u16]) {
    let s = cdef::CDEF_BSTRIDE;
    let very = cdef::CDEF_VERY_LARGE;
    // The 8x8 block lives at rows VBORDER..VBORDER+8, cols HBORDER..HBORDER+8
    // of a (8 + 2*VBORDER) x (8 + HBORDER*2 + ...) region; taps reach 2.
    let rows = 8 + 2 * cdef::CDEF_VBORDER;
    let cols = 8 + 2 * cdef::CDEF_HBORDER;
    let mut fill = |r0: usize, r1: usize, c0: usize, c1: usize| {
        for r in r0..r1 {
            for c in c0..c1 {
                buf[r * s + c] = very;
            }
        }
    };
    match pattern {
        0 => {}                                              // interior: no sentinels
        1 => fill(0, cdef::CDEF_VBORDER, 0, cols),           // frame top
        2 => fill(rows - cdef::CDEF_VBORDER, rows, 0, cols), // frame bottom
        3 => fill(0, rows, 0, cdef::CDEF_HBORDER),           // frame left
        4 => fill(0, rows, cdef::CDEF_HBORDER + 8, cols),    // frame right
        5 => {
            // top-left frame corner
            fill(0, cdef::CDEF_VBORDER, 0, cols);
            fill(0, rows, 0, cdef::CDEF_HBORDER);
        }
        6 => {
            // bottom-right frame corner
            fill(rows - cdef::CDEF_VBORDER, rows, 0, cols);
            fill(0, rows, cdef::CDEF_HBORDER + 8, cols);
        }
        _ => {
            // adversarial: random sentinel scatter in the halo only
            for r in 0..rows {
                for c in 0..cols {
                    let in_block = (cdef::CDEF_VBORDER..cdef::CDEF_VBORDER + 8).contains(&r)
                        && (cdef::CDEF_HBORDER..cdef::CDEF_HBORDER + 8).contains(&c);
                    if !in_block && rng.range(3) == 0 {
                        buf[r * s + c] = very;
                    }
                }
            }
        }
    }
}

fn random_pixels(rng: &mut Rng, buf: &mut [u16]) {
    for v in buf.iter_mut() {
        *v = rng.byte() as u16;
    }
}

/// Compare our 16-bit-input filter kernel against C for one parameter set.
#[allow(clippy::too_many_arguments)]
fn check_filter_block(
    inb: &[u16],
    pri: i32,
    sec: i32,
    dir: i32,
    pri_damping: i32,
    sec_damping: i32,
    bsize: i32,
    coeff_shift: i32,
    sub: usize,
    what: &str,
) {
    let mut ours = [0xAAu8; 64];
    let mut theirs = [0xAAu8; 64];
    cdef::cdef_filter_block(
        &mut ours,
        0,
        8,
        inb,
        IOFF,
        pri,
        sec,
        dir,
        pri_damping,
        sec_damping,
        bsize,
        coeff_shift,
        sub,
    );
    cref::cdef_filter_block_8(
        &mut theirs,
        0,
        8,
        inb,
        IOFF,
        pri,
        sec,
        dir,
        pri_damping,
        sec_damping,
        bsize,
        coeff_shift,
        sub as u8,
    );
    assert_eq!(
        ours, theirs,
        "filter_block diverges: {what} pri {pri} sec {sec} dir {dir} \
         damping {pri_damping}/{sec_damping} bsize {bsize} shift {coeff_shift} sub {sub}"
    );
}

/// Every syntax-signalable strength combination, over every border pattern:
/// packed strength 0..=63 -> pri = s/4, sec = s%4 (+1 when 3, the decode
/// rule), luma damping 3..=6 and the chroma-derived 2..=5, dirs 0..=7,
/// luma (8x8) and 420-chroma (4x4) block shapes.
#[test]
fn filter_block_all_signalable_combos() {
    let mut rng = Rng(0xF117E7);
    let mut inb = vec![0u16; cdef::CDEF_INBUF_SIZE];
    for pattern in 0..8u32 {
        random_pixels(&mut rng, &mut inb);
        apply_borders(pattern, &mut rng, &mut inb);
        for packed in 0..64i32 {
            let pri = packed / cdef::CDEF_SEC_STRENGTHS;
            let mut sec = packed % cdef::CDEF_SEC_STRENGTHS;
            sec += i32::from(sec == 3);
            for damping in 2..=6i32 {
                // dir varies; use a couple per (strength, damping) to bound
                // runtime, all 8 dirs hit across the packed sweep.
                for dir in [(packed & 7), ((packed >> 1) ^ 5) & 7] {
                    for bsize in [cdef::BLOCK_8X8, cdef::BLOCK_4X4] {
                        check_filter_block(
                            &inb,
                            pri,
                            sec,
                            dir,
                            damping,
                            damping,
                            bsize,
                            0,
                            1,
                            border_label(pattern),
                        );
                    }
                }
            }
        }
    }
}

fn border_label(pattern: u32) -> &'static str {
    match pattern {
        0 => "interior",
        1 => "top",
        2 => "bottom",
        3 => "left",
        4 => "right",
        5 => "top-left",
        6 => "bottom-right",
        _ => "scatter",
    }
}

/// Randomized wide sweep: independent pri/sec dampings, all four block
/// shapes, decimated rows (sub=2), HBD coeff_shift domain, and raw-u16
/// torture content (pins the int16 wrap/cast semantics).
#[test]
fn filter_block_randomized_wide() {
    let mut rng = Rng(0xD1DE_57EE7);
    let mut inb = vec![0u16; cdef::CDEF_INBUF_SIZE];
    let bsizes = [
        cdef::BLOCK_4X4,
        cdef::BLOCK_4X8,
        cdef::BLOCK_8X4,
        cdef::BLOCK_8X8,
    ];
    for round in 0..2000u32 {
        let torture = round % 10 == 9;
        if torture {
            for v in inb.iter_mut() {
                *v = rng.next() as u16;
            }
        } else {
            random_pixels(&mut rng, &mut inb);
            apply_borders(rng.range(8) as u32, &mut rng, &mut inb);
        }
        let coeff_shift = if round % 7 == 6 { 2 } else { 0 };
        let pri = (rng.range(16) as i32) << coeff_shift;
        let sec = ([0, 1, 2, 4][rng.range(4) as usize]) << coeff_shift;
        let dir = rng.range(8) as i32;
        let pd = 2 + rng.range(5) as i32 + coeff_shift;
        let sd = 2 + rng.range(5) as i32 + coeff_shift;
        let bsize = bsizes[rng.range(4) as usize];
        let sub = 1 + rng.range(2) as usize;
        check_filter_block(
            &inb,
            pri,
            sec,
            dir,
            pd,
            sd,
            bsize,
            coeff_shift,
            sub,
            if torture { "torture-u16" } else { "random" },
        );
    }
}

/// The native 8-bit interior kernel (no sentinel) against C, same coverage.
#[test]
fn filter_block_8bit_matches_c() {
    let mut rng = Rng(0x8B17F117);
    let mut inb = vec![0u8; cdef::CDEF_INBUF_SIZE];
    let bsizes = [
        cdef::BLOCK_4X4,
        cdef::BLOCK_4X8,
        cdef::BLOCK_8X4,
        cdef::BLOCK_8X8,
    ];
    for round in 0..3000u32 {
        for v in inb.iter_mut() {
            *v = rng.byte();
        }
        let pri = rng.range(16) as i32;
        let sec = [0, 1, 2, 4][rng.range(4) as usize];
        let dir = rng.range(8) as i32;
        let damping = 2 + rng.range(5) as i32;
        let bsize = bsizes[(round % 4) as usize];
        let sub = 1 + rng.range(2) as usize;
        let mut ours = [0x55u8; 64];
        let mut theirs = [0x55u8; 64];
        cdef::cdef_filter_block_8bit(
            &mut ours, 0, 8, &inb, IOFF, pri, sec, dir, damping, bsize, 0, sub,
        );
        cref::cdef_filter_block_8bit(
            &mut theirs,
            0,
            8,
            &inb,
            IOFF,
            pri,
            sec,
            dir,
            damping,
            bsize,
            0,
            sub as u8,
        );
        assert_eq!(
            ours, theirs,
            "8bit kernel diverges: pri {pri} sec {sec} dir {dir} damping {damping} \
             bsize {bsize} sub {sub}"
        );
    }
}

// =============================================================================
// HIGHBD (is_16bit / 10-bit pipeline) arms.
//
// These two kernels are what the bd10 CDEF strength SEARCH runs per filter
// block: `svt_cdef_filter_fb` at `is_16bit` calls `svt_cdef_filter_block_c`
// with `dst8 = NULL, dst16 = tmp_dst` (cdef_process.c:527-528), then
// `compute_cdef_dist` dispatches to `svt_aom_compute_cdef_dist_16bit_c`
// (cdef_process.c:246). Both were previously untested — `cdef_filter_block_hbd`
// carried a `PORT-NOTE(unverified)` and the dist kernel had no hbd port at all.
// They are load-bearing for the bd10 search, so they are differentially
// pinned against the REAL exported C symbols here.
// =============================================================================

/// The `dst16` store arm of `svt_cdef_filter_block_c`, over the same
/// randomized/border/torture coverage as the `dst8` arm above, with the
/// coeff_shift = 2 (10-bit) domain oversampled — that is the only domain in
/// which this arm is reachable.
#[test]
fn filter_block_hbd_dst16_matches_c() {
    let mut rng = Rng(0x16B1_7DEF);
    let mut inb = vec![0u16; cdef::CDEF_INBUF_SIZE];
    let bsizes = [
        cdef::BLOCK_4X4,
        cdef::BLOCK_4X8,
        cdef::BLOCK_8X4,
        cdef::BLOCK_8X8,
    ];
    let mut seen_shift2 = 0u32;
    for round in 0..3000u32 {
        let torture = round % 10 == 9;
        if torture {
            for v in inb.iter_mut() {
                *v = rng.next() as u16;
            }
        } else {
            // 10-bit content: pixels in 0..=1023, plus the CDEF_VERY_LARGE
            // sentinel borders the search's `build_src` writes off-frame.
            for v in inb.iter_mut() {
                *v = (rng.next() % 1024) as u16;
            }
            apply_borders(rng.range(8) as u32, &mut rng, &mut inb);
        }
        // coeff_shift 2 dominates (the bd10 domain); 0 kept as a control.
        let coeff_shift = if round % 5 == 4 { 0 } else { 2 };
        if coeff_shift == 2 {
            seen_shift2 += 1;
        }
        let pri = (rng.range(16) as i32) << coeff_shift;
        let sec = ([0, 1, 2, 4][rng.range(4) as usize]) << coeff_shift;
        let dir = rng.range(8) as i32;
        // C: `damping += coeff_shift - (pli != PLANE_Y)`, damping base 3..=6.
        let pd = 3 + rng.range(4) as i32 + coeff_shift - i32::from(round % 3 == 0);
        let sd = pd;
        let bsize = bsizes[rng.range(4) as usize];
        let sub = 1 + rng.range(2) as usize;

        let mut ours = [0xAAAAu16; 64];
        let mut theirs = [0xAAAAu16; 64];
        svtav1_dsp::hbd::cdef_filter_block_hbd(
            &mut ours,
            0,
            8,
            &inb,
            IOFF,
            pri,
            sec,
            dir,
            pd,
            sd,
            bsize,
            coeff_shift,
            sub,
        );
        cref::cdef_filter_block_16(
            &mut theirs,
            0,
            8,
            &inb,
            IOFF,
            pri,
            sec,
            dir,
            pd,
            sd,
            bsize,
            coeff_shift,
            sub as u8,
        );
        assert_eq!(
            ours.as_slice(),
            theirs.as_slice(),
            "hbd dst16 kernel diverges: round {round} pri {pri} sec {sec} dir {dir} \
             damping {pd}/{sd} bsize {bsize} shift {coeff_shift} sub {sub} \
             ({})",
            if torture { "torture-u16" } else { "10-bit" }
        );
    }
    assert!(seen_shift2 > 2000, "coverage: coeff_shift=2 must dominate");
}

/// `svt_aom_compute_cdef_dist_16bit_c` (enc_cdef.c:77) — the bd10 search's
/// per-filter-block distortion, including the `sum >> 2 * coeff_shift`
/// normalization back to the 8-bit scale.
#[test]
fn compute_cdef_dist_16bit_matches_c() {
    let mut rng = Rng(0xD157_16B1);
    // Plane the SOURCE is read from (C's `dst` parameter) and the packed
    // filtered blocks (C's `src`).
    let plane_w = 128usize;
    let mut plane = vec![0u16; plane_w * 128];
    let mut packed = vec![0u16; 64 * 64];
    for (bsize, dim) in [
        (cdef::BLOCK_8X8, 8usize),
        (cdef::BLOCK_4X4, 4),
        (cdef::BLOCK_4X8, 4),
        (cdef::BLOCK_8X4, 8),
    ] {
        for round in 0..250u32 {
            for v in plane.iter_mut() {
                *v = (rng.next() % 1024) as u16;
            }
            for v in packed.iter_mut() {
                *v = (rng.next() % 1024) as u16;
            }
            let count = 1 + rng.range(16) as usize;
            let dlist: Vec<(u8, u8)> = (0..count)
                .map(|i| ((i / 4) as u8, (i % 4) as u8))
                .collect();
            let coeff_shift = if round % 4 == 3 { 0 } else { 2 };
            let sub = if bsize == cdef::BLOCK_4X4 {
                1
            } else {
                1 + rng.range(2) as u8
            };
            let plane_off = (rng.range(8) as usize) * plane_w + rng.range(8) as usize;
            let ours = svtav1_dsp::cdef::compute_cdef_dist_16bit(
                &plane,
                plane_off,
                plane_w,
                &packed,
                &dlist,
                bsize,
                coeff_shift,
                sub as usize,
            );
            let theirs = cref::compute_cdef_dist_16bit(
                &plane,
                plane_off,
                plane_w,
                &packed,
                &dlist,
                bsize,
                coeff_shift,
                sub,
            );
            assert_eq!(
                ours, theirs,
                "cdef dist 16bit diverges: bsize {bsize} (dim {dim}) count {count} \
                 shift {coeff_shift} sub {sub} off {plane_off}"
            );
        }
    }
}

/// The 8-bit twin of the same kernel — the port's existing `dist_packed`
/// was hand-inlined in the encoder with no differential; this pins the
/// shared implementation the bd8 search now routes through, so the bd10
/// wiring cannot silently change bd8 behaviour.
#[test]
fn compute_cdef_dist_8bit_matches_c() {
    let mut rng = Rng(0xD157_08B1);
    let plane_w = 128usize;
    let mut plane = vec![0u8; plane_w * 128];
    let mut packed = vec![0u8; 64 * 64];
    for bsize in [
        cdef::BLOCK_8X8,
        cdef::BLOCK_4X4,
        cdef::BLOCK_4X8,
        cdef::BLOCK_8X4,
    ] {
        for round in 0..250u32 {
            for v in plane.iter_mut() {
                *v = rng.byte();
            }
            for v in packed.iter_mut() {
                *v = rng.byte();
            }
            let count = 1 + rng.range(16) as usize;
            let dlist: Vec<(u8, u8)> = (0..count)
                .map(|i| ((i / 4) as u8, (i % 4) as u8))
                .collect();
            let sub = if bsize == cdef::BLOCK_4X4 {
                1
            } else {
                1 + rng.range(2) as u8
            };
            let plane_off = (rng.range(8) as usize) * plane_w + rng.range(8) as usize;
            let _ = round;
            let ours = svtav1_dsp::cdef::compute_cdef_dist_8bit(
                &plane,
                plane_off,
                plane_w,
                &packed,
                &dlist,
                bsize,
                0,
                sub as usize,
            );
            let theirs =
                cref::compute_cdef_dist_8bit(&plane, plane_off, plane_w, &packed, &dlist, bsize, 0, sub);
            assert_eq!(
                ours, theirs,
                "cdef dist 8bit diverges: bsize {bsize} count {count} sub {sub} off {plane_off}"
            );
        }
    }
}

// =============================================================================
// Dispatch-tier lock for the SIMD filter (task G4).
//
// `cdef_filter_block` (dst8) and `cdef_filter_block_hbd` (dst16) gained an AVX2
// path. The exhaustive suites above run whatever tier this host dispatches to
// (v3 here), so they already pin v3==C — but to guarantee EVERY tier (the scalar
// reference AND the AVX2 kernel) stays byte-identical to real C regardless of the
// host, force each token permutation and assert equality with the C reference for
// every combination. Both arms cover the cols==8 SIMD shapes AND the 4-wide scalar
// fallback, interior/border/sentinel/torture content, and the bd10 coeff_shift=2
// domain.
// =============================================================================
#[test]
fn filter_block_dispatch_all_tiers_match_c() {
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
    let mut rng = Rng(0xD15A_7C4D);
    let mut inb = vec![0u16; cdef::CDEF_INBUF_SIZE];
    let bsizes = [
        cdef::BLOCK_8X8, // cols 8 -> AVX2 path
        cdef::BLOCK_8X4, // cols 8 -> AVX2 path (rows 4)
        cdef::BLOCK_4X8, // cols 4 -> scalar fallback
        cdef::BLOCK_4X4, // cols 4 -> scalar fallback
    ];
    for round in 0..96u32 {
        let torture = round % 8 == 7;
        if torture {
            for v in inb.iter_mut() {
                *v = rng.next() as u16;
            }
        } else {
            random_pixels(&mut rng, &mut inb);
            apply_borders(rng.range(8) as u32, &mut rng, &mut inb);
        }
        let coeff_shift = if round % 3 == 0 { 2 } else { 0 };
        let pri = (rng.range(16) as i32) << coeff_shift;
        let sec = ([0, 1, 2, 4][rng.range(4) as usize]) << coeff_shift;
        let dir = rng.range(8) as i32;
        let pd = 2 + rng.range(5) as i32 + coeff_shift;
        let sd = 2 + rng.range(5) as i32 + coeff_shift;
        let bsize = bsizes[rng.range(4) as usize];
        let sub = 1 + rng.range(2) as usize;

        // dst8 arm: C reference, then port under EVERY dispatch tier.
        let mut theirs8 = [0xAAu8; 64];
        cref::cdef_filter_block_8(
            &mut theirs8, 0, 8, &inb, IOFF, pri, sec, dir, pd, sd, bsize, coeff_shift, sub as u8,
        );
        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut ours = [0xAAu8; 64];
            cdef::cdef_filter_block(
                &mut ours, 0, 8, &inb, IOFF, pri, sec, dir, pd, sd, bsize, coeff_shift, sub,
            );
            assert_eq!(
                ours, theirs8,
                "dst8 dispatch tier != C: round {round} bsize {bsize} pri {pri} sec {sec} \
                 dir {dir} sub {sub} shift {coeff_shift}"
            );
        });

        // dst16 (bd10) arm: same.
        let mut theirs16 = [0xBBBBu16; 64];
        cref::cdef_filter_block_16(
            &mut theirs16, 0, 8, &inb, IOFF, pri, sec, dir, pd, sd, bsize, coeff_shift, sub as u8,
        );
        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut ours = [0xBBBBu16; 64];
            svtav1_dsp::hbd::cdef_filter_block_hbd(
                &mut ours, 0, 8, &inb, IOFF, pri, sec, dir, pd, sd, bsize, coeff_shift, sub,
            );
            assert_eq!(
                ours.as_slice(),
                theirs16.as_slice(),
                "dst16 dispatch tier != C: round {round} bsize {bsize} pri {pri} sec {sec} \
                 dir {dir} sub {sub} shift {coeff_shift}"
            );
        });
    }
}

/// Sign-extension guard. The C kernel reads the `uint16_t*` input into `int16_t`
/// locals (`cdef.c:205`), so values ≥ 0x8000 wrap negative; a SIMD load that
/// zero-extends diverges. Neither uniform-high content (all pixels shift by
/// 65536 ≡ 0 mod 256, so it cancels) nor random torture (huge diffs saturate
/// `constrain` to 0) exposes this — the distinguisher is content that STRADDLES
/// 0x8000 with small u16 gaps: zero-extension then sees a small, `constrain`-active
/// diff where the correct int16 view sees a huge, saturated one. Combined with a
/// large strength (so the damped clamp stays active on the small gap), the two
/// interpretations produce different pixels. This test pins the correct
/// (sign-extending) path against real C on exactly that content; it FAILS if the
/// vector load is switched to zero-extension (verified). Purely a semantics lock —
/// real CDEF input is always ≤ 0x7f7f (pixels + the 0x7f7f sentinel), where
/// sign- and zero-extension coincide.
#[test]
fn filter_block_sign_straddle_matches_c() {
    let mut rng = Rng(0x516E_E871);
    let mut inb = vec![0u16; cdef::CDEF_INBUF_SIZE];
    for round in 0..800u32 {
        // Each pixel lands just below OR just above 0x8000 (gap ≤ 63), so many
        // tap/center pairs straddle the sign boundary.
        for v in inb.iter_mut() {
            let off = (rng.next() % 64) as u16;
            *v = if rng.range(2) == 0 {
                0x7FC0u16.wrapping_add(off).min(0x7FFF)
            } else {
                0x8000u16.wrapping_add(off)
            };
        }
        let bsize = cdef::BLOCK_8X8; // cols==8 -> AVX2 path
        // Large strengths keep `constrain` active on the small zero-ext gap
        // (thr << shift > gap), so the sign flip changes the output.
        let pri = 12 + rng.range(4) as i32;
        let sec = [2, 4][rng.range(2) as usize];
        let dir = rng.range(8) as i32;
        let pd = 6;
        let sd = 6;
        let sub = 1 + rng.range(2) as usize;

        let mut ours8 = [0u8; 64];
        let mut c8 = [0u8; 64];
        cdef::cdef_filter_block(&mut ours8, 0, 8, &inb, IOFF, pri, sec, dir, pd, sd, bsize, 0, sub);
        cref::cdef_filter_block_8(&mut c8, 0, 8, &inb, IOFF, pri, sec, dir, pd, sd, bsize, 0, sub as u8);
        assert_eq!(
            ours8, c8,
            "dst8 sign-straddle != C: round {round} pri {pri} sec {sec} dir {dir} sub {sub}"
        );

        let mut ours16 = [0u16; 64];
        let mut c16 = [0u16; 64];
        svtav1_dsp::hbd::cdef_filter_block_hbd(
            &mut ours16, 0, 8, &inb, IOFF, pri, sec, dir, pd, sd, bsize, 0, sub,
        );
        cref::cdef_filter_block_16(
            &mut c16, 0, 8, &inb, IOFF, pri, sec, dir, pd, sd, bsize, 0, sub as u8,
        );
        assert_eq!(
            ours16.as_slice(),
            c16.as_slice(),
            "dst16 sign-straddle != C: round {round} pri {pri} sec {sec} dir {dir} sub {sub}"
        );
    }
}
