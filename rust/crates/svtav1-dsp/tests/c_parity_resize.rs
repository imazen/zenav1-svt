//! Differential gate: the superres SOURCE downscale vs the real C
//! (`svt_av1_resize_plane_horizontal` -> `resize_multistep` ->
//! {`svt_av1_down2_symeven`, `down2_symodd`, `choose_interp_filter` +
//! `svt_av1_interpolate_core`}, resize.c).
//!
//! This side is NOT normative — the decoder never runs it — but byte-parity
//! with C requires reproducing it exactly: a different downscale means a
//! different SOURCE, hence different modes, residuals and bytes. So it gets
//! the same treatment as the normative upscale.
//!
//! Covered: all four band-limited filter banks plus the normative (1000) bank
//! via `choose_interp_filter`'s ladder, the exact-2:1 `down2_symeven` arm
//! (denominator 16), odd lengths (`down2_symodd`, reached through the
//! multistep driver), the short-input `x1 > x2` regime, and every superres
//! denominator 9..=16 over several widths and content classes.

use svtav1_cref as cref;
use svtav1_dsp::resize::{
    FILTEREDINTERP_FILTERS500, FILTEREDINTERP_FILTERS625, FILTEREDINTERP_FILTERS750,
    FILTEREDINTERP_FILTERS875, choose_interp_filter, down2_length, down2_steps, down2_symeven,
    interpolate_core, resize_multistep, resize_plane_horizontal,
};
use svtav1_dsp::superres::{RESIZE_FILTER_NORMATIVE, scaled_size};

fn lcg(seed: u32) -> impl FnMut() -> u8 {
    let mut s = seed;
    move || {
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (s >> 24) as u8
    }
}

fn contents(width: usize) -> Vec<(&'static str, Vec<u8>)> {
    let mut r = lcg(0x9E37_79B9);
    vec![
        (
            "step",
            (0..width)
                .map(|c| if c < width / 2 { 16u8 } else { 235 })
                .collect(),
        ),
        (
            "ramp",
            (0..width).map(|c| (c * 255 / width.max(1)) as u8).collect(),
        ),
        ("noise", (0..width).map(|_| r()).collect()),
        (
            "checker",
            (0..width)
                .map(|c| if (c / 3) % 2 == 0 { 0u8 } else { 255 })
                .collect(),
        ),
    ]
}

/// The four transcribed filter banks are byte-identical to C's, and
/// `choose_interp_filter`'s ladder selects the same bank C does.
#[test]
fn filter_banks_and_ladder_match_c() {
    // Bank identity is proven through the kernel: running `interpolate_core`
    // with each bank index against C's same-bank call over random input would
    // pass even on a table typo only if the typo were unreachable, so drive
    // every phase by using a long line at a ratio that sweeps all 64 phases.
    let mut r = lcg(0xDEAD_BEEF);
    let src: Vec<u8> = (0..257).map(|_| r()).collect();
    let banks: [(&str, &[[i16; 8]; 64], i32); 5] = [
        ("1000", &RESIZE_FILTER_NORMATIVE, 0),
        ("875", &FILTEREDINTERP_FILTERS875, 1),
        ("750", &FILTEREDINTERP_FILTERS750, 2),
        ("625", &FILTEREDINTERP_FILTERS625, 3),
        ("500", &FILTEREDINTERP_FILTERS500, 4),
    ];
    for (name, table, idx) in banks {
        for &out_len in &[91usize, 129, 200] {
            let mut got = vec![0u8; out_len];
            interpolate_core(&src, src.len(), &mut got, out_len, table);
            let mut expect = vec![0u8; out_len];
            cref::interpolate_core(&src, src.len(), &mut expect, out_len, idx);
            assert_eq!(got, expect, "bank {name} -> {out_len}");
        }
    }
    // The ladder: C `choose_interp_filter` thresholds on out*16 vs in*{16,13,11,9}.
    assert!(core::ptr::eq(
        choose_interp_filter(100, 100),
        &RESIZE_FILTER_NORMATIVE
    ));
    assert!(core::ptr::eq(
        choose_interp_filter(100, 90),
        &FILTEREDINTERP_FILTERS875
    ));
    assert!(core::ptr::eq(
        choose_interp_filter(100, 75),
        &FILTEREDINTERP_FILTERS750
    ));
    assert!(core::ptr::eq(
        choose_interp_filter(100, 60),
        &FILTEREDINTERP_FILTERS625
    ));
    assert!(core::ptr::eq(
        choose_interp_filter(100, 40),
        &FILTEREDINTERP_FILTERS500
    ));
}

/// The exact-2:1 decimation kernel matches C at even and odd lengths, short
/// and long (the `l1 > l2` short-input branch).
#[test]
fn down2_symeven_matches_c() {
    let mut r = lcg(0x0BAD_F00D);
    for &len in &[4usize, 6, 8, 10, 16, 33, 64, 65, 128, 257] {
        let src: Vec<u8> = (0..len).map(|_| r()).collect();
        let out_len = len.div_ceil(2);
        let mut got = vec![0u8; out_len];
        down2_symeven(&src, len, &mut got);
        let mut expect = vec![0u8; out_len];
        cref::down2_symeven(&src, len, &mut expect);
        assert_eq!(got, expect, "down2_symeven len {len}");
    }
}

/// End-to-end: the plane downscale is byte-identical to C for every superres
/// denominator, several widths (even and odd), and four content classes.
/// This drives C's `static` multistep driver — including the `down2_symodd`
/// arm — so a divergence anywhere in the ladder surfaces here.
#[test]
fn resize_plane_horizontal_matches_c_across_denominators() {
    let mut cells = 0usize;
    let rows = 3usize;
    for &frame_w in &[64usize, 96, 127, 128, 176, 255, 256, 512] {
        for denom in 9u8..=16 {
            let coded_w = scaled_size(frame_w as u16, denom) as usize;
            for (name, line) in contents(frame_w) {
                // Give each row distinct content so a row-indexing bug shows.
                let mut src = vec![0u8; frame_w * rows];
                for r in 0..rows {
                    for c in 0..frame_w {
                        src[r * frame_w + c] = line[c].wrapping_add((r * 29) as u8);
                    }
                }
                let mut got = vec![0u8; coded_w * rows];
                resize_plane_horizontal(&src, rows, frame_w, frame_w, &mut got, coded_w, coded_w);
                let mut expect = vec![0u8; coded_w * rows];
                cref::resize_plane_horizontal(
                    &src,
                    rows,
                    frame_w,
                    frame_w,
                    &mut expect,
                    coded_w,
                    coded_w,
                );
                assert_eq!(
                    got, expect,
                    "frame_w {frame_w} denom {denom} ({coded_w}) content {name}"
                );
                cells += 1;
            }
        }
    }
    assert!(cells >= 250, "sweep too thin: {cells} cells");
    println!("resize plane horizontal: {cells} cells byte-identical to C");
}

/// Denominator 16 is the only superres ratio that takes an exact 2:1 step
/// (C's comment: "only denom 16 will come here and steps equals to 1"), so the
/// down2 arm is genuinely exercised by the sweep above — not dead code.
#[test]
fn denom_16_takes_the_down2_step() {
    for &frame_w in &[64usize, 128, 256] {
        let coded_w = scaled_size(frame_w as u16, 16) as usize;
        assert_eq!(
            down2_steps(frame_w, coded_w),
            1,
            "denom 16 at width {frame_w} must take exactly one 2:1 step"
        );
        assert_eq!(down2_length(frame_w, 1), coded_w);
        for denom in 9u8..=15 {
            let cw = scaled_size(frame_w as u16, denom) as usize;
            assert_eq!(
                down2_steps(frame_w, cw),
                0,
                "denom {denom} at width {frame_w} must go straight to interpolate"
            );
        }
    }
}

/// Anti-vacuity: the downscale is a real filter, not a decimating pick — and
/// a 1:1 "resize" is the identity copy (C's `length == olength` early out).
#[test]
fn resize_is_filtering_not_picking() {
    let width = 128usize;
    let coded = scaled_size(width as u16, 12) as usize;
    let line: Vec<u8> = (0..width)
        .map(|c| if (c / 2) % 2 == 0 { 0u8 } else { 255 })
        .collect();
    let mut out = vec![0u8; coded];
    resize_multistep(&line, width, &mut out, coded);
    assert!(
        out.iter().any(|&v| v != 0 && v != 255),
        "a band-limited downscale of a checker must produce intermediate values"
    );
    let mut same = vec![0u8; width];
    resize_multistep(&line, width, &mut same, width);
    assert_eq!(same, line, "1:1 resize is the identity");
}
