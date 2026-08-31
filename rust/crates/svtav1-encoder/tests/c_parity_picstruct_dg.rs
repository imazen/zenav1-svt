//! Differential test for the dynamic-GOP detector's HME segment against the
//! REAL exported C symbol `dg_detector_hme_level0` — evidence **tier 1**
//! (`docs/WORKING-ON-THIS.md` §4).
//!
//! Why this one matters: `enable_dg` is 1 for single-pass CQP/CRF
//! `RANDOM_ACCESS` below 4K (`enc_handle.c:4294-4300`), so this detector runs
//! BY DEFAULT there, and its output decides the mini-GOP SIZE — i.e. the
//! temporal layer of every frame in it. A one-SAD difference flips the
//! decision, which is why this is a differential and not a traced vector.
//!
//! It exercises `early_hme_b64` (and through it `sad_loop_kernel`) as well,
//! since that is the only way into the search from here — `early_hme_b64` was
//! INLINED into `dg_detector_hme_level0` by the C compiler and has no symbol
//! of its own (`nm` on `pd_process.c.o` finds only `early_hme`), so there is
//! no finer-grained differential available.
//!
//! **What this gate can and cannot detect, mutation-tested rather than
//! assumed.** Perturbing `metrics.tot_dist += sad` to `+= sad + 1` fails it at
//! the first cell, so the comparison is live. Three other perturbations do
//! NOT fail it, and the reason is a property of the C interface rather than a
//! hole in the grid:
//!
//!   * scaling `sr_center.x`/`.y` by 2 instead of 4 — the metrics read only
//!     the SIGN and the non-zero-ness of the centre, never its magnitude, so
//!     no observable here depends on the scale;
//!
//!   * moving the `tot_cplx` threshold from `16*16*30` to `16*16*31` — the
//!     sweep DOES cross that threshold (a positive control asserts it), but no
//!     SAD in this corpus lands in the 256-wide window between the two values;
//!
//!   * dropping the round-up of `sa_width` to a multiple of 8 — every search
//!     width this detector uses (16, 64, 128) is already a multiple of 8.
//!
//! The first of those is un-gateable from this entry point and is stated as a
//! limit rather than papered over.

use svtav1_cref::picstruct as cref;
use svtav1_encoder::port_picstruct as pp;

/// A padded plane: `border` pixels on every side, filled with a deterministic
/// pattern, so the search can read the negative offsets it wants.
struct Plane {
    data: Vec<u8>,
    origin: usize,
    stride: usize,
    width: u16,
    height: u16,
    border: u16,
}

/// A padded plane filled with a deterministic pattern.
///
/// `kind` picks the pattern and `(dx, dy)` shifts it, so a reference built
/// with the same seed and a non-zero shift is a TRANSLATED copy of the source
/// — which is what makes the search return a non-zero motion vector instead of
/// sitting at the centre. A grid that never moves the search centre cannot
/// detect a bug in the centre arithmetic; measured, see the positive controls.
fn make_plane(
    width: u16,
    height: u16,
    border: u16,
    seed: u32,
    kind: u8,
    dx: i32,
    dy: i32,
) -> Plane {
    let stride = width as usize + 2 * border as usize;
    let rows = height as usize + 2 * border as usize;
    let mut data = vec![0u8; stride * rows];
    for row in 0..rows {
        for col in 0..stride {
            let x = col as i32 - i32::from(border) + dx;
            let y = row as i32 - i32::from(border) + dy;
            let v = match kind {
                // Flat: every SAD is equal, so the kernel keeps its first best.
                0 => 128u8,
                // Sharp blobs on a plain field: a unique, unambiguous match.
                1 => {
                    let bx = x.rem_euclid(24);
                    let by = y.rem_euclid(24);
                    if (4..12).contains(&bx) && (4..12).contains(&by) {
                        240
                    } else {
                        16
                    }
                }
                // Deterministic hash: a rough SAD surface with near-ties,
                // which is where a tie-break difference would show up.
                _ => {
                    let mut h = (x as u32)
                        .wrapping_mul(0x9E37_79B9)
                        .wrapping_add((y as u32).wrapping_mul(0x85EB_CA6B))
                        .wrapping_add(seed);
                    h ^= h >> 15;
                    h = h.wrapping_mul(0x2545_F491);
                    h ^= h >> 13;
                    (h >> 24) as u8
                }
            };
            data[row * stride + col] = v;
        }
    }
    Plane {
        data,
        origin: border as usize * stride + border as usize,
        stride,
        width,
        height,
        border,
    }
}

#[test]
fn c_parity_dg_detector_hme_level0() {
    // (aligned_width, aligned_height) at full resolution; the planes are the
    // sixteenth-downsampled ones, i.e. a quarter per axis.
    let cases: [(u32, u32, u8); 6] = [
        // 320x192 -> 80x48 downsampled, 360p range (search area 16x16).
        (320, 192, 0),
        (320, 192, 1),
        (320, 192, 2),
        // 640x384 -> 160x96, 480p range (search area 64x64).
        (640, 384, 2),
        // 1280x704 -> 320x176, above 480p (search area 128x128).
        (1280, 704, 2),
        // A single 64x64 block: the smallest picture the detector can walk,
        // and the one where the skipped middle row/column matters most.
        (64, 64, 2),
    ];

    let mut saw_nonzero_dist = false;
    let mut saw_active = false;
    let mut saw_nonzero_balance = false;
    let mut saw_cplx = false;

    for (aligned_w, aligned_h, kind) in cases {
        for input_resolution in [0u8, 1, 2, 4] {
            for (seg_cols, seg_rows) in [(1u32, 1u32), (2, 1), (1, 2), (2, 2)] {
                let ds_w = (aligned_w / 4) as u16;
                let ds_h = (aligned_h / 4) as u16;
                // The C search reads up to `border - 1` pixels outside the
                // picture; 64 covers the widest search area (128 wide, half
                // each side) at the picture edges.
                let border = 64u16;

                for seg_idx in 0..(seg_cols * seg_rows) {
                    for (dx, dy) in [(0i32, 0i32), (3, 0), (0, -2), (5, 7), (-9, 4)] {
                        let mut src = make_plane(ds_w, ds_h, border, 0x1234_5678, kind, 0, 0);
                        // The reference is the SAME pattern translated, so the
                        // best match sits at a known non-zero offset.
                        let mut rf = make_plane(ds_w, ds_h, border, 0x1234_5678, kind, dx, dy);

                        // Port side.
                        let src_view = pp::DsPlane {
                            data: &src.data,
                            origin: src.origin,
                            stride: src.stride,
                            width: src.width,
                            height: src.height,
                            border: src.border,
                        };
                        let ref_view = pp::DsPlane {
                            data: &rf.data,
                            origin: rf.origin,
                            stride: rf.stride,
                            width: rf.width,
                            height: rf.height,
                            border: rf.border,
                        };
                        let mut got = pp::DgDetectorMetrics::default();
                        pp::dg_detector_hme_level0(
                            &mut got,
                            &src_view,
                            &ref_view,
                            input_resolution,
                            aligned_w,
                            aligned_h,
                            64,
                            seg_idx,
                            seg_cols,
                            seg_rows,
                        );

                        // C side.
                        let mut c_src = cref::DgPlane {
                            data: &mut src.data,
                            origin: src.origin as u32,
                            stride: src.stride as u32,
                            width: src.width,
                            height: src.height,
                            border: src.border,
                        };
                        let mut c_ref = cref::DgPlane {
                            data: &mut rf.data,
                            origin: rf.origin as u32,
                            stride: rf.stride as u32,
                            width: rf.width,
                            height: rf.height,
                            border: rf.border,
                        };
                        let want = cref::dg_detector_hme_level0(
                            &mut c_src,
                            &mut c_ref,
                            input_resolution,
                            aligned_w,
                            aligned_h,
                            64,
                            seg_idx,
                            seg_cols,
                            seg_rows,
                        );

                        let ctx = format!(
                            "dg_detector_hme_level0({aligned_w}x{aligned_h}, kind={kind}, \
                         res={input_resolution}, segs={seg_cols}x{seg_rows}, seg={seg_idx}, \
                         shift=({dx},{dy}))"
                        );
                        assert_eq!(got.tot_dist, want.tot_dist, "{ctx} tot_dist");
                        assert_eq!(got.tot_cplx, want.tot_cplx, "{ctx} tot_cplx");
                        assert_eq!(got.tot_active, want.tot_active, "{ctx} tot_active");
                        assert_eq!(
                            got.sum_in_vectors, want.sum_in_vectors,
                            "{ctx} sum_in_vectors"
                        );
                        assert_eq!(got.seg_completed, want.seg_completed, "{ctx} seg_completed");

                        if want.tot_dist != 0 {
                            saw_nonzero_dist = true;
                        }
                        if want.tot_active != 0 {
                            saw_active = true;
                        }
                        if want.sum_in_vectors != 0 {
                            saw_nonzero_balance = true;
                        }
                        if want.tot_cplx != 0 {
                            saw_cplx = true;
                        }
                    }
                }
            }
        }
    }

    // Positive controls. Without these an all-zeros port would agree with an
    // all-zeros probe on every cell (WORKING-ON-THIS.md §5): the sweep must
    // actually produce distortion, actives and a non-zero inward/outward
    // balance somewhere.
    assert!(saw_nonzero_dist, "the sweep never produced a non-zero SAD");
    assert!(
        saw_active,
        "the sweep never produced a non-zero motion vector"
    );
    assert!(
        saw_nonzero_balance,
        "the sweep never produced a non-zero sum_in_vectors"
    );
    assert!(
        saw_cplx,
        "the sweep never crossed the tot_cplx threshold (16*16*30)"
    );
}
