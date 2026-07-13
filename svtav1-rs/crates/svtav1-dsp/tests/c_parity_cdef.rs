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
            assert_eq!((rd, rv), (cd, cv), "find_dir_8bit diverges, stride {stride}");
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
        0 => {} // interior: no sentinels
        1 => fill(0, cdef::CDEF_VBORDER, 0, cols),          // frame top
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
            &mut theirs, 0, 8, &inb, IOFF, pri, sec, dir, damping, bsize, 0, sub as u8,
        );
        assert_eq!(
            ours, theirs,
            "8bit kernel diverges: pri {pri} sec {sec} dir {dir} damping {damping} \
             bsize {bsize} sub {sub}"
        );
    }
}
