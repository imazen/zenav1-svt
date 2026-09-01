//! The TWO-dimensional plane resize (`Codec/resize.c:422` and `:725`) against
//! the REAL exported C symbols — evidence **tier 1**
//! (`docs/WORKING-ON-THIS.md` §4).
//!
//! `nm -g Bin/Release/libSvtAv1Enc.a` reports `T _svt_av1_resize_plane_c` and
//! `T _svt_av1_highbd_resize_plane_c`. Both drive C's `static`
//! `resize_multistep` / `highbd_resize_multistep` — the down2 ladder plus the
//! polyphase interpolator — through BOTH the row pass and the column pass, so
//! this one differential covers the vertical arm those kernels had never been
//! exercised on: the port previously had only the horizontal-only variant that
//! superres needs.
//!
//! **The oracle is pinned to C's `_c` dispatch tier.** Those exported symbols
//! are not pure C on x86-64: `resize_multistep` reaches its leaves through the
//! RTCD pointers, and the AVX2 kernels emit a fixed-width block regardless of
//! the requested length — so below length 34 they overrun the caller and
//! disagree with their own `_c` twins (`docs/SUSPECTED-C-BUGS.md` #26,
//! measured). `aom_dsp_rtcd.c`'s AARCH64 arm is `SET_ONLY_C` for every resize
//! symbol, so aarch64 gets the `_c` ladder for free; `shims/refmgmt_shims.c`
//! pins x86-64 to the same one, which is what `SVT_CPU_FLAGS=0` does globally.
//! Without that pin these cells are host-dependent: `64x48 -> 16x12` differs
//! in exactly its last column, and the highbd/frame cells abort in glibc's
//! heap checker. The AVX2 behaviour is not hidden — it is asserted as measured
//! in `c_parity_resize_avx2_divergence.rs`.

use svtav1_cref::ref_mgmt as cref;

/// A deterministic pseudo-random plane, so a failure is reproducible from the
/// dimensions alone.
fn plane(w: usize, stride: usize, h: usize, seed: u32, max: u16) -> Vec<u16> {
    let mut s = seed | 1;
    let mut v = vec![0u16; h * stride];
    for row in 0..h {
        for col in 0..w {
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
            v[row * stride + col] = ((s >> 16) % u32::from(max + 1)) as u16;
        }
    }
    v
}

/// The dimension pairs. Every superres/resize denominator 8..=16 maps a source
/// dimension to `dim * 8 / denom` rounded, plus the exact powers of two that
/// take the down2 ladder and the odd sizes that do not.
fn cells() -> Vec<(usize, usize, usize, usize)> {
    let mut out = Vec::new();
    for &(w, h) in &[(64usize, 48usize), (96, 64), (176, 144), (65, 33), (17, 5)] {
        for &denom in &[9u32, 10, 12, 14, 16] {
            let w2 = ((w as u32 * 8 + denom / 2) / denom).max(1) as usize;
            let h2 = ((h as u32 * 8 + denom / 2) / denom).max(1) as usize;
            out.push((w, h, w2, h2));
        }
        // Exact halves and quarters take the down2 ladder rather than the
        // polyphase filter, which is a different arm of `resize_multistep`.
        out.push((w, h, w / 2, h / 2));
        out.push((w, h, (w / 4).max(1), (h / 4).max(1)));
        // Vertical-only and horizontal-only degenerate cases.
        out.push((w, h, w, h / 2));
        out.push((w, h, w / 2, h));
        out.push((w, h, w, h));
    }
    out
}

/// TIER 1, 8-bit. Padded strides on BOTH sides, because the column pass is the
/// half that has never been driven and a stride bug there is invisible when
/// stride equals width.
#[test]
fn c_parity_resize_plane_2d() {
    let mut compared = 0usize;
    let mut nontrivial = 0usize;
    for (w, h, w2, h2) in cells() {
        let in_stride = w + 7;
        let out_stride = w2 + 5;
        let src16 = plane(w, in_stride, h, 0x1234_5678, 255);
        let src: Vec<u8> = src16.iter().map(|&v| v as u8).collect();

        let mut want = vec![0xAAu8; h2 * out_stride];
        let mut got = vec![0xAAu8; h2 * out_stride];
        cref::resize_plane(&src, h, w, in_stride, &mut want, h2, w2, out_stride);
        svtav1_dsp::resize::resize_plane(&src, h, w, in_stride, &mut got, h2, w2, out_stride);

        for r in 0..h2 {
            assert_eq!(
                &got[r * out_stride..r * out_stride + w2],
                &want[r * out_stride..r * out_stride + w2],
                "{w}x{h} -> {w2}x{h2}, row {r}"
            );
            // The bytes PAST the resized width must be untouched, which is
            // what a scatter with the wrong stride would break.
            assert_eq!(
                &got[r * out_stride + w2..(r + 1) * out_stride],
                &want[r * out_stride + w2..(r + 1) * out_stride],
                "{w}x{h} -> {w2}x{h2}, row {r} padding"
            );
        }
        compared += 1;
        if h2 != h {
            nontrivial += 1;
        }
    }
    assert!(compared > 30, "only {compared} cells compared");
    assert!(
        nontrivial > 20,
        "only {nontrivial} cells actually changed the HEIGHT, so the column pass was barely exercised"
    );
}

/// TIER 1, 10-bit, at both `bd` values the encoder ships.
#[test]
fn c_parity_highbd_resize_plane_2d() {
    let mut compared = 0usize;
    for bd in [10i32, 12] {
        let max = if bd == 10 { 1023u16 } else { 4095 };
        for (w, h, w2, h2) in cells() {
            let in_stride = w + 3;
            let out_stride = w2 + 9;
            let src = plane(w, in_stride, h, 0x9E37_79B9, max);

            let mut want = vec![0x5555u16; h2 * out_stride];
            let mut got = vec![0x5555u16; h2 * out_stride];
            cref::highbd_resize_plane(&src, h, w, in_stride, &mut want, h2, w2, out_stride, bd);
            svtav1_dsp::port_resize_hbd::highbd_resize_plane(
                &src, h, w, in_stride, &mut got, h2, w2, out_stride, bd,
            );

            for r in 0..h2 {
                assert_eq!(
                    &got[r * out_stride..r * out_stride + w2],
                    &want[r * out_stride..r * out_stride + w2],
                    "bd{bd} {w}x{h} -> {w2}x{h2}, row {r}"
                );
            }
            compared += 1;
        }
    }
    assert!(compared > 60, "only {compared} cells compared");
}

/// The 2-D resize at an unchanged height must equal the horizontal-only
/// variant the superres path already uses — a cross-check between two ports of
/// the same C ladder that would catch a column pass that corrupts rows.
#[test]
fn two_d_at_equal_height_matches_the_horizontal_only_path() {
    for (w, h, w2) in [(64usize, 48usize, 48usize), (176, 144, 96), (65, 33, 33)] {
        let in_stride = w + 4;
        let out_stride = w2 + 2;
        let src16 = plane(w, in_stride, h, 0xDEAD_BEEF, 255);
        let src: Vec<u8> = src16.iter().map(|&v| v as u8).collect();

        let mut two_d = vec![0u8; h * out_stride];
        let mut horiz = vec![0u8; h * out_stride];
        svtav1_dsp::resize::resize_plane(&src, h, w, in_stride, &mut two_d, h, w2, out_stride);
        svtav1_dsp::resize::resize_plane_horizontal(
            &src, h, w, in_stride, &mut horiz, w2, out_stride,
        );
        for r in 0..h {
            assert_eq!(
                &two_d[r * out_stride..r * out_stride + w2],
                &horiz[r * out_stride..r * out_stride + w2],
                "{w}x{h} -> {w2}, row {r}"
            );
        }
    }
}

/// TIER 1. `svt_aom_resize_frame` (`resize.c:881`) — the whole-frame plane
/// loop, 4:2:0 and 4:4:4.
///
/// Two decisions live only here and nowhere below: the per-plane dimensions
/// (`(dim + ss) >> ss`, which rounds UP, so an ODD luma dimension gives the
/// LARGER chroma plane) and the kernel selection (horizontal-only iff the
/// FRAME heights match — not the plane's, which can differ from it in 4:2:0
/// at an odd height). Both are driven here, including the odd-dimension cells
/// that separate them.
#[test]
fn c_parity_resize_frame() {
    let cases: &[(usize, usize, usize, usize, u32, u32)] = &[
        // 4:2:0, even dims, both dimensions scaled.
        (64, 48, 48, 36, 1, 1),
        (176, 144, 96, 80, 1, 1),
        // 4:2:0 at ODD dims: chroma rounds UP, and an odd height makes the
        // chroma heights differ where the luma heights would not.
        (65, 33, 49, 25, 1, 1),
        (17, 5, 9, 3, 1, 1),
        // Height UNCHANGED: C takes the horizontal-only kernel for every
        // plane, including chroma.
        (64, 48, 48, 48, 1, 1),
        (65, 33, 33, 33, 1, 1),
        // 4:4:4 — chroma is full size, so the shifts are inert.
        (64, 48, 32, 24, 0, 0),
        (65, 33, 49, 25, 0, 0),
        // Identity.
        (64, 48, 64, 48, 1, 1),
    ];

    let mut compared = 0usize;
    let mut saw_horizontal_only = false;
    let mut saw_two_d = false;
    let mut saw_chroma_round_up = false;

    for &(sw, sh, dw, dh, ss_x, ss_y) in cases {
        let strides = |w: usize, ss: u32| (w + 6, ((w + ss as usize) >> ss) + 4);
        let (s_ys, s_cs) = strides(sw, ss_x);
        let (d_ys, d_cs) = strides(dw, ss_x);
        let (s_cw, s_ch) = svtav1_dsp::resize::plane_dims(1, sw, sh, ss_x, ss_y);
        let (_d_cw, d_ch) = svtav1_dsp::resize::plane_dims(1, dw, dh, ss_x, ss_y);
        saw_chroma_round_up |= sw % 2 == 1 && ss_x == 1 && s_cw * 2 > sw;
        saw_horizontal_only |= sh == dh;
        saw_two_d |= sh != dh;

        let src_y: Vec<u8> = plane(sw, s_ys, sh, 0x0BAD_F00D, 255)
            .iter()
            .map(|&v| v as u8)
            .collect();
        let src_u: Vec<u8> = plane(s_cw, s_cs, s_ch, 0x1357_9BDF, 255)
            .iter()
            .map(|&v| v as u8)
            .collect();
        let src_v: Vec<u8> = plane(s_cw, s_cs, s_ch, 0x2468_ACE0, 255)
            .iter()
            .map(|&v| v as u8)
            .collect();

        let mut want = (
            vec![0x77u8; dh * d_ys],
            vec![0x77u8; d_ch * d_cs],
            vec![0x77u8; d_ch * d_cs],
        );
        let mut got = want.clone();

        cref::resize_frame(
            &mut [
                cref::CrefFramePlane {
                    src: &src_y,
                    src_stride: s_ys,
                    dst: &mut want.0,
                    dst_stride: d_ys,
                },
                cref::CrefFramePlane {
                    src: &src_u,
                    src_stride: s_cs,
                    dst: &mut want.1,
                    dst_stride: d_cs,
                },
                cref::CrefFramePlane {
                    src: &src_v,
                    src_stride: s_cs,
                    dst: &mut want.2,
                    dst_stride: d_cs,
                },
            ],
            sw,
            sh,
            dw,
            dh,
            2,
            ss_x,
            ss_y,
        );

        svtav1_dsp::resize::resize_frame(
            &mut [
                svtav1_dsp::resize::FramePlane {
                    src: &src_y,
                    src_stride: s_ys,
                    dst: &mut got.0,
                    dst_stride: d_ys,
                },
                svtav1_dsp::resize::FramePlane {
                    src: &src_u,
                    src_stride: s_cs,
                    dst: &mut got.1,
                    dst_stride: d_cs,
                },
                svtav1_dsp::resize::FramePlane {
                    src: &src_v,
                    src_stride: s_cs,
                    dst: &mut got.2,
                    dst_stride: d_cs,
                },
            ],
            sw,
            sh,
            dw,
            dh,
            ss_x,
            ss_y,
        );

        assert_eq!(got.0, want.0, "{sw}x{sh} -> {dw}x{dh} ss({ss_x},{ss_y}): Y");
        assert_eq!(got.1, want.1, "{sw}x{sh} -> {dw}x{dh} ss({ss_x},{ss_y}): U");
        assert_eq!(got.2, want.2, "{sw}x{sh} -> {dw}x{dh} ss({ss_x},{ss_y}): V");
        compared += 1;
    }

    assert_eq!(compared, cases.len());
    assert!(
        saw_horizontal_only,
        "the height-unchanged kernel arm was never taken"
    );
    assert!(saw_two_d, "the two-dimensional kernel arm was never taken");
    assert!(
        saw_chroma_round_up,
        "no odd-width 4:2:0 cell, so the chroma round-up was never checked"
    );
}
