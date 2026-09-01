//! C's AVX2 resize leaf disagrees with its own `_c` twin — evidence **tier 1**
//! (`docs/WORKING-ON-THIS.md` §4), and this file PINS the measurement.
//!
//! `svt_av1_resize_plane_c` is an exported symbol, but on x86-64 it is not
//! pure C: `resize_multistep` reaches its leaves through the RTCD pointers,
//! and `aom_dsp_rtcd.c:602` binds `svt_av1_down2_symeven` to
//! `svt_av1_down2_symeven_avx2`. That kernel processes a FIXED 32-sample
//! initial block and emits a FIXED 16 outputs before any length test
//! (`ASM_AVX2/resize_avx2.c:822`, `down2_symeven_w16_init_part_avx2` at
//! `:501`), so below length 34 it both writes past the caller's buffer and
//! skips the high-edge clamp its `_c` twin applies. `aom_dsp_rtcd.c:1368`
//! binds the same name to `_c` on aarch64 — there is no Neon resize kernel —
//! which is why the divergence is invisible there.
//!
//! The plane differentials (`c_parity_resize_plane_2d.rs`) therefore pin C's
//! resize dispatch to the `_c` tier, so the oracle is the ladder the port
//! ports on both hosts. This file is the counterweight: it reaches the AVX2
//! symbol DIRECTLY and asserts the divergence exactly as measured, so the
//! pinning can never quietly become a story nobody rechecks. **If a cell here
//! starts failing, upstream changed the kernel — re-measure and re-decide;
//! do not widen the assertion.**
//!
//! Measured 2026-08-31, x86-64 Linux (Ryzen 9 7900X, glibc, gcc), against
//! `Bin/Release/libSvtAv1Enc.a` at `42da724d`.
//!
//! See `docs/SUSPECTED-C-BUGS.md` #20.

#![cfg(target_arch = "x86_64")]

use svtav1_cref::ref_mgmt as cref;

/// Padding the AVX2 kernel's fixed-width access needs so the MEASUREMENT
/// itself stays in bounds. The overrun is the thing being documented; it must
/// not be reproduced by this harness against a too-small buffer.
const IN_PAD: usize = 64;
const OUT_CAP: usize = 64;

fn source(len: usize) -> Vec<u8> {
    (0..len + IN_PAD)
        .map(|i| ((i * 37 + 11) & 0xFF) as u8)
        .collect()
}

/// Which output indices the kernel actually touched.
///
/// Two runs with opposite canaries, unioned: a byte that comes back equal to
/// both `0x00` and `0xFF` cannot exist, so no real output value can be
/// mistaken for an untouched cell.
fn written_extent(len: usize) -> usize {
    let src = source(len);
    let mut lo = vec![0x00u8; OUT_CAP];
    let mut hi = vec![0xFFu8; OUT_CAP];
    cref::avx2_down2_symeven(&src, len, &mut lo);
    cref::avx2_down2_symeven(&src, len, &mut hi);
    let mut last = 0usize;
    for i in 0..OUT_CAP {
        if lo[i] != 0x00 || hi[i] != 0xFF {
            last = i + 1;
        }
    }
    last
}

fn avx2_and_c(len: usize) -> (Vec<u8>, Vec<u8>) {
    let src = source(len);
    let mut a = vec![0u8; OUT_CAP];
    let mut c = vec![0u8; OUT_CAP];
    cref::avx2_down2_symeven(&src, len, &mut a);
    cref::c_down2_symeven(&src, len, &mut c);
    (a, c)
}

/// The positive control (`docs/WORKING-ON-THIS.md` §5: prove the probe fires
/// before you trust anything it reports).
#[test]
fn the_avx2_leaf_is_reachable_on_this_host() {
    assert!(
        std::arch::is_x86_feature_detected!("avx2"),
        "this x86-64 host has no AVX2, so C's own encoder would not take the \
         kernel this file measures — the control cannot run and must not pass \
         silently"
    );
    assert!(
        cref::resize_has_avx2_leaves(),
        "the cref shim did not expose the AVX2 resize leaf on x86-64"
    );
    // A length the two implementations AGREE on, so a green result below is
    // never just "the harness compares nothing".
    let (a, c) = avx2_and_c(64);
    assert_eq!(a[..32], c[..32], "the two kernels disagree at length 64");
    assert_eq!(written_extent(64), 32, "length 64 should write exactly 32");
}

/// Below length 34 the AVX2 kernel writes a FIXED 16 outputs, whatever the
/// caller asked for. `svt_av1_resize_plane_c`'s column pass hands it an
/// `arrbuf2` of exactly `height2` bytes (`resize.c:440`), so any ladder step
/// under 32 samples overruns a heap allocation by `16 - length / 2` bytes.
#[test]
fn the_avx2_leaf_writes_sixteen_outputs_below_length_thirty_four() {
    for len in (2..=32).step_by(2) {
        assert_eq!(
            written_extent(len),
            16,
            "length {len}: expected the measured fixed-width 16-output write"
        );
    }
    for len in (34..=128).step_by(2) {
        assert_eq!(
            written_extent(len),
            len / 2,
            "length {len}: expected exactly len/2 outputs"
        );
    }
}

/// And the VALUES diverge, because the fixed initial block never applies the
/// high-edge clamp `svt_av1_down2_symeven_c` applies in its end part.
/// Length 2 is the one short cell whose single output happens to agree.
#[test]
fn the_avx2_leaf_values_diverge_from_c_below_length_thirty_four() {
    let mut diverged = Vec::new();
    for len in (2..=32).step_by(2) {
        let (a, c) = avx2_and_c(len);
        if a[..len / 2] != c[..len / 2] {
            diverged.push(len);
        }
    }
    assert_eq!(
        diverged,
        (4..=32).step_by(2).collect::<Vec<_>>(),
        "the set of lengths whose VALUES diverge moved from what was measured"
    );

    // The exact cell that took `c_parity_resize_plane_2d` red on x86-64: the
    // 64 -> 16 row ladder's second step is `down2_symeven(_, 32, _)`, and only
    // its LAST output differs.
    let (a, c) = avx2_and_c(32);
    assert_eq!(
        a[..15],
        c[..15],
        "only the last output should differ at length 32"
    );
    assert_ne!(a[15], c[15], "the last output at length 32 should differ");

    for len in (34..=128).step_by(2) {
        let (a, c) = avx2_and_c(len);
        assert_eq!(
            a[..len / 2],
            c[..len / 2],
            "length {len} was measured as agreeing"
        );
    }
}
