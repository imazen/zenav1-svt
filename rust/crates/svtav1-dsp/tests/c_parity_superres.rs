//! Differential gate: the NORMATIVE super-resolution upscale vs the real C
//! (`upscale_normative_rect` -> `av1_convolve_horiz_rs_c` with
//! `svt_av1_resize_filter_normative`, super_res.c — unchanged 4.1 -> 4.2).
//!
//! Superres upscaling is NORMATIVE: the decoder runs this exact filter, so an
//! approximation is a decode mismatch, not a quality trade. `superres.rs` used
//! to carry a 16-phase Q14 stub and this suite PINNED the divergence
//! (`assert_ne!`); the chunk-A port replaced it with the real kernel and these
//! are now byte-identity asserts across the whole denominator range.
//!
//! Covered: every filter phase, the `x_step_qn` / `x0_qn` geometry (implicitly
//! — a wrong `x0` shifts the entire row), the tap/border reach at both frame
//! edges, and denominators 9..=16 (C's `SUPERRES_SCALE_DENOMINATOR_MIN` ..
//! `SCALE_DENOMINATOR_MAX`) over several widths and content classes.

use svtav1_cref as cref;
use svtav1_dsp::superres::{
    RESIZE_FILTER_NORMATIVE, TileColPad, UPSCALE_BORDER_COLS, scaled_size, upscale_convolve_step,
    upscale_convolve_x0, upscale_normative_plane, upscale_normative_row,
};

/// The transcribed 64x8 table is byte-identical to C's, entry for entry.
#[test]
fn normative_filter_table_matches_c() {
    for phase in 0..64usize {
        let c = cref::superres_filter_normative(phase);
        assert_eq!(
            RESIZE_FILTER_NORMATIVE[phase], c,
            "resize_filter_normative[{phase}]"
        );
    }
    // Anti-vacuity: the table genuinely resolves 64 phases (a 16-phase table,
    // like the stub this replaced, would repeat every 4 rows).
    assert_eq!(RESIZE_FILTER_NORMATIVE[0], [0, 0, 0, 128, 0, 0, 0, 0]);
    assert_ne!(
        RESIZE_FILTER_NORMATIVE[1], RESIZE_FILTER_NORMATIVE[4],
        "64-phase resolution"
    );
}

/// Content classes that stress the kernel differently: a hard step edge (max
/// ringing from the negative taps), a ramp, pseudo-random noise, and extremes
/// pinned at both edges (border replication).
fn contents(width: usize) -> Vec<(&'static str, Vec<u8>)> {
    let mut lcg: u32 = 0x1234_5678;
    let mut rand = move || {
        lcg = lcg.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (lcg >> 24) as u8
    };
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
        ("noise", (0..width).map(|_| rand()).collect()),
        (
            "edges",
            (0..width)
                .map(|c| if c == 0 || c + 1 == width { 255 } else { 0 })
                .collect(),
        ),
    ]
}

/// Run the C oracle on one row (it needs >= 5 border bytes each side, which it
/// replicates and then restores in place).
fn c_upscale(row: &[u8], out_width: usize) -> Vec<u8> {
    let border = UPSCALE_BORDER_COLS;
    let mut padded = vec![0u8; row.len() + 2 * border];
    padded[border..border + row.len()].copy_from_slice(row);
    let mut out = vec![0u8; out_width];
    cref::superres_upscale_row(&mut padded, border, row.len(), &mut out, out_width);
    // The rect restores what it overwrote — a port that mutated its input
    // would be a different function.
    assert_eq!(&padded[border..border + row.len()], row, "input restored");
    out
}

/// Port the same way C's row driver does (step/x0 from the plane widths).
fn port_upscale(row: &[u8], out_width: usize) -> Vec<u8> {
    let in_w = row.len();
    let step = upscale_convolve_step(in_w as i32, out_width as i32);
    let x0 = upscale_convolve_x0(in_w as i32, out_width as i32, step);
    let mut out = vec![0u8; out_width];
    upscale_normative_row(
        row,
        0,
        in_w,
        &mut out,
        out_width,
        step,
        x0,
        TileColPad::FRAME,
    );
    out
}

/// The ported row kernel is byte-identical to C across every superres
/// denominator, several frame widths, and four content classes.
#[test]
fn upscale_row_matches_c_across_denominators() {
    let mut cells = 0usize;
    for &frame_w in &[64usize, 96, 128, 176, 256, 320, 512] {
        for denom in 9u8..=16 {
            let coded_w = scaled_size(frame_w as u16, denom) as usize;
            assert!(
                coded_w > 0 && coded_w <= frame_w,
                "denom {denom}: coded {coded_w} vs frame {frame_w}"
            );
            for (name, row) in contents(coded_w) {
                assert_eq!(
                    port_upscale(&row, frame_w),
                    c_upscale(&row, frame_w),
                    "frame_w {frame_w} denom {denom} content {name}: coded_w {coded_w}"
                );
                cells += 1;
            }
        }
    }
    assert!(cells >= 200, "sweep too thin: {cells} cells");
    println!("superres upscale row: {cells} cells byte-identical to C");
}

/// Anti-vacuity for the sweep above: the upscale genuinely filters — it is
/// neither a no-op nor a nearest-neighbour copy.
#[test]
fn upscale_is_not_degenerate() {
    let frame_w = 128usize;
    let coded_w = scaled_size(frame_w as u16, 12) as usize;
    let row: Vec<u8> = (0..coded_w)
        .map(|c| if c < coded_w / 2 { 16u8 } else { 235 })
        .collect();
    let out = c_upscale(&row, frame_w);
    assert!(
        out.iter().any(|&v| v != 16 && v != 235),
        "an 8-tap upscale of a step edge must produce intermediate values"
    );
    assert_eq!(port_upscale(&row, frame_w), out);
}

/// The whole-plane driver applies the row kernel to every row (the shape the
/// encoder's recon-upscale hook needs), matching C row by row.
#[test]
fn upscale_plane_matches_c_row_by_row() {
    let (frame_w, rows) = (192usize, 5usize);
    let coded_w = scaled_size(frame_w as u16, 11) as usize;
    let mut src = vec![0u8; coded_w * rows];
    for r in 0..rows {
        for c in 0..coded_w {
            src[r * coded_w + c] = ((r * 37 + c * 5) % 256) as u8;
        }
    }
    let mut dst = vec![0u8; frame_w * rows];
    upscale_normative_plane(&src, coded_w, coded_w, &mut dst, frame_w, frame_w, rows);
    for r in 0..rows {
        let expect = c_upscale(&src[r * coded_w..(r + 1) * coded_w], frame_w);
        assert_eq!(
            &dst[r * frame_w..(r + 1) * frame_w],
            &expect[..],
            "plane row {r}"
        );
    }
}

/// `scaled_size` reproduces C `calculate_scaled_size_helper`: denom 8 is the
/// identity (no superres) and the result is clamped to >= min(16, dim).
#[test]
fn scaled_size_matches_c_rule() {
    for w in [16u16, 64, 96, 128, 320, 1920, 3840] {
        assert_eq!(scaled_size(w, 8), w, "denom 8 is unscaled");
        for denom in 9u8..=16 {
            let got = scaled_size(w, denom);
            let expect = core::cmp::max(
                (u32::from(w) * 8 + u32::from(denom) / 2) / u32::from(denom),
                core::cmp::min(16, u32::from(w)),
            ) as u16;
            assert_eq!(got, expect, "w {w} denom {denom}");
            assert!(got <= w, "downscale only: {got} vs {w}");
        }
    }
    // Tiny frames keep their size rather than dropping below the spec floor.
    assert_eq!(scaled_size(8, 16), 8, "min_dim = min(16, dim) floor");
}
