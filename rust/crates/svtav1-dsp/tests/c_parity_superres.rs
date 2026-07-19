//! Differential audit: super-resolution upscale vs the C normative reference
//! (`upscale_normative_rect` -> `av1_convolve_horiz_rs_c` with
//! `svt_av1_resize_filter_normative`, super_res.c). super_res.c did not change
//! 4.1->4.2.
//!
//! FINDING — `superres.rs::superres_upscale_row` is a STUB, not a port of the C
//! normative upscale:
//!   * C uses `svt_av1_resize_filter_normative` — RS_SUBPEL_BITS=6 => 64 phases;
//!     Rust embeds a 16-phase table (`SUPERRES_FILTER`).
//!   * C accumulates the source position in the RS_SCALE_SUBPEL_BITS=14 domain
//!     with `av1_get_upscale_convolve_step` / `get_upscale_convolve_x0`
//!     (initial-offset-aware); Rust uses `(src_width<<14)/dst_width` with a
//!     4-bit phase and no initial offset.
//!   * C replicates frame-edge borders (`upscale_normative_rect`); Rust clamps
//!     indices.
//!
//! This suite (a) proves the C oracle is callable and its filter table has true
//! 64-phase resolution, and (b) PINS the divergence from the stub. A real port
//! must adopt the 64-phase table + RS_SCALE phase math; then `assert_ne!`
//! becomes `assert_eq!`.

use svtav1_cref as cref;
use svtav1_dsp::superres::superres_upscale_row;

/// The C normative filter genuinely resolves 64 phases: adjacent phase rows
/// differ (a 16-phase table cannot represent this), and phase 0 is the identity
/// tap `[0,0,0,128,0,0,0,0]`.
#[test]
fn c_normative_filter_has_64_phase_resolution() {
    let p0 = cref::superres_filter_normative(0);
    assert_eq!(p0, [0, 0, 0, 128, 0, 0, 0, 0], "phase 0 is identity");
    // Distinct adjacent phases across the full 64-entry range.
    for p in 0..63usize {
        let a = cref::superres_filter_normative(p);
        let b = cref::superres_filter_normative(p + 1);
        assert_ne!(a, b, "phases {p} and {} must differ (64-phase table)", p + 1);
    }
    // A 16-phase table would repeat every 4 rows; the normative one does not.
    assert_ne!(
        cref::superres_filter_normative(1),
        cref::superres_filter_normative(4),
        "64-phase resolution (not a 16-phase table)"
    );
}

/// GAP-PIN: a 2x normative upscale diverges from the Rust stub. Flip to
/// `assert_eq!` when superres.rs adopts the normative kernel.
#[test]
fn superres_upscale_diverges_from_c_on_2x() {
    let in_width = 8usize;
    let out_width = 16usize;
    let row: [u8; 8] = [30, 90, 40, 200, 120, 60, 180, 75];

    // C oracle: pad the row with >=5 border bytes each side (rect restores them).
    let border = 5usize;
    let mut padded = vec![0u8; in_width + 2 * border];
    padded[border..border + in_width].copy_from_slice(&row);
    let mut out_c = vec![0u8; out_width];
    cref::superres_upscale_row(&mut padded, border, in_width, &mut out_c, out_width);
    assert!(out_c.iter().any(|&v| v != out_c[0]), "C upscale non-degenerate");
    // Borders were restored in place.
    assert_eq!(&padded[border..border + in_width], &row, "input restored");

    // Rust stub.
    let mut out_rust = vec![0u8; out_width];
    superres_upscale_row(&row, in_width, &mut out_rust, out_width);

    assert_ne!(
        out_rust, out_c,
        "STUB pinned: superres_upscale_row is not the normative upscale \
         (see module doc). When ported, change this to assert_eq!"
    );
}
