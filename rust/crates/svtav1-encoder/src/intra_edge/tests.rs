use super::*;
use alloc::vec::Vec;

/// 128x128 test frame with a recognizable per-pixel pattern.
fn test_frame() -> (Vec<u8>, usize) {
    let stride = 128;
    let mut recon = alloc::vec![0u8; stride * 128];
    for y in 0..128usize {
        for x in 0..128usize {
            recon[y * stride + x] = ((y * 7 + x * 3) & 0xFF) as u8;
        }
    }
    (recon, stride)
}

const P_NONE: PartitionType = PartitionType::None;

// ---- has_top_right (64x64 SB, whole-block tx, luma) ----

#[test]
fn has_tr_top_row_of_sb_available() {
    // 32x32 at mi(16, 8): top row of its SB → available.
    assert!(has_top_right(
        16, 32, 32, 16, 8, true, true, P_NONE, 8, 0, 0, 0, 0
    ));
}

#[test]
fn has_tr_rightmost_col_of_sb_unavailable() {
    // 32x32 at mi(8, 8): bottom-right quadrant → rightmost column of
    // SB and not the top row → unavailable.
    assert!(!has_top_right(
        16, 32, 32, 8, 8, true, true, P_NONE, 8, 0, 0, 0, 0
    ));
}

#[test]
fn has_tr_bottom_left_quadrant_from_table() {
    // 32x32 at mi(8, 0): bottom-left quadrant. TR pixels are the
    // bottom row of the top-right quadrant, already coded →
    // has_tr_32x32[0] bit 4 = 1 (95 = 0b0101_1111).
    assert!(has_top_right(
        16, 32, 32, 8, 0, true, true, P_NONE, 8, 0, 0, 0, 0
    ));
}

#[test]
fn has_tr_unavailable_without_top_or_right() {
    assert!(!has_top_right(
        16, 32, 32, 8, 0, false, true, P_NONE, 8, 0, 0, 0, 0
    ));
    assert!(!has_top_right(
        16, 32, 32, 8, 0, true, false, P_NONE, 8, 0, 0, 0, 0
    ));
}

#[test]
fn has_tr_16x16_interior() {
    // 16x16 at mi(4, 4): this_blk_index = (1 << 3) + 1 = 9 →
    // has_tr_16x16[1] = 85 = 0b0101_0101, bit 1 = 0 → unavailable.
    assert!(!has_top_right(
        16, 16, 16, 4, 4, true, true, P_NONE, 4, 0, 0, 0, 0
    ));
    // 16x16 at mi(4, 8): index = (1 << 3) + 2 = 10 → bit 2 of 85 = 1.
    assert!(has_top_right(
        16, 16, 16, 4, 8, true, true, P_NONE, 4, 0, 0, 0, 0
    ));
}

// ---- has_bottom_left ----

#[test]
fn has_bl_leftmost_col_within_sb() {
    // 16x16 at mi(4, 0): leftmost column of SB, BL pixels rows within
    // the SB → row_off_in_sb (4) + 4 < 16 → available.
    assert!(has_bottom_left(
        16, 16, 16, 4, 0, true, true, P_NONE, 4, 0, 0, 0, 0
    ));
    // 32x32 at mi(8, 0): 8 + 8 < 16 is false → unavailable.
    assert!(!has_bottom_left(
        16, 32, 32, 8, 0, true, true, P_NONE, 8, 0, 0, 0, 0
    ));
}

#[test]
fn has_bl_16x16_top_right_area_from_table() {
    // 16x16 at mi(0, 8): index = (0 << 3) + 2 = 2 →
    // has_bl_16x16[0] = 84 = 0b0101_0100, bit 2 = 1 → available
    // (BL pixels are the TL 32x32 quadrant's right edge, coded first).
    assert!(has_bottom_left(
        16, 16, 16, 0, 8, true, true, P_NONE, 4, 0, 0, 0, 0
    ));
    // 16x16 at mi(4, 4): index = (1 << 3) + 1 = 9 →
    // has_bl_16x16[1] = 16 = 0b0001_0000, bit 1 = 0 → unavailable.
    assert!(!has_bottom_left(
        16, 16, 16, 4, 4, true, true, P_NONE, 4, 0, 0, 0, 0
    ));
}

#[test]
fn has_bl_bottom_row_of_sb_unavailable() {
    // 16x16 at mi(12, 4): bottom row of SB, not leftmost col → 0.
    assert!(!has_bottom_left(
        16, 16, 16, 12, 4, true, true, P_NONE, 4, 0, 0, 0, 0
    ));
}

// ---- build_directional_edges ----

#[test]
fn edges_z1_real_top_right_when_available() {
    let (recon, stride) = test_frame();
    // 32x32 at (64, 64): mi(16, 16), top row of SB(1,1) → TR available;
    // xr = 128 - 96 = 32 → n_topright = 32 real pixels.
    match build_directional_edges(&recon, stride, 64, 64, 32, 32, 45, P_NONE, 16) {
        DirEdges::Edges { above, .. } => {
            for i in 0..64 {
                assert_eq!(
                    above[i],
                    recon[63 * stride + 64 + i],
                    "above[{i}] must be the real reconstructed pixel"
                );
            }
        }
        DirEdges::Flat(_) => panic!("expected edges"),
    }
}

#[test]
fn edges_z1_replicates_when_top_right_unavailable() {
    let (recon, stride) = test_frame();
    // 32x32 at (96, 64): mi(16, 24) → rightmost column of SB(1,1),
    // not its top row? blk_row_in_sb = (16 & 15) >> 3 = 0 → TOP row →
    // available, but xr = 128 - 128 = 0 → right_available false →
    // has_top_right = 0 → replicate above[31].
    match build_directional_edges(&recon, stride, 96, 64, 32, 32, 45, P_NONE, 16) {
        DirEdges::Edges { above, .. } => {
            for i in 0..32 {
                assert_eq!(above[i], recon[63 * stride + 96 + i]);
            }
            let last = recon[63 * stride + 127];
            for i in 32..64 {
                assert_eq!(
                    above[i], last,
                    "above[{i}] must replicate the last real pixel"
                );
            }
        }
        DirEdges::Flat(_) => panic!("expected edges"),
    }
}

#[test]
fn edges_z1_no_top_flat_fill_from_left() {
    let (recon, stride) = test_frame();
    // 32x32 at (32, 0): no top row, angle 45 needs above only →
    // C early exit: flat fill with left_ref[0] = recon[0*128 + 31].
    match build_directional_edges(&recon, stride, 32, 0, 32, 32, 45, P_NONE, 16) {
        DirEdges::Flat(v) => assert_eq!(v, recon[31]),
        DirEdges::Edges { .. } => panic!("expected C's flat-fill early exit"),
    }
}

#[test]
fn edges_frame_corner_defaults() {
    let (recon, stride) = test_frame();
    // Block at (0,0): z1 → flat 127; z3 → flat 129; z2 → 127/129/128.
    match build_directional_edges(&recon, stride, 0, 0, 32, 32, 45, P_NONE, 16) {
        DirEdges::Flat(v) => assert_eq!(v, 127),
        _ => panic!("expected flat"),
    }
    match build_directional_edges(&recon, stride, 0, 0, 32, 32, 203, P_NONE, 16) {
        DirEdges::Flat(v) => assert_eq!(v, 129),
        _ => panic!("expected flat"),
    }
    match build_directional_edges(&recon, stride, 0, 0, 32, 32, 135, P_NONE, 16) {
        DirEdges::Edges {
            above,
            left,
            top_left,
        } => {
            assert!(above.iter().all(|&v| v == 127));
            assert!(left.iter().all(|&v| v == 129));
            assert_eq!(top_left, 128);
        }
        DirEdges::Flat(_) => panic!("z2 needs both edges; no early exit"),
    }
}

#[test]
fn edges_z3_real_bottom_left_when_available() {
    let (recon, stride) = test_frame();
    // 16x16 at (32, 0): mi(0, 8) → has_bl_16x16 bit 2 = 1, yd = 112 →
    // n_bottomleft = 16 real pixels below the block at col 31.
    match build_directional_edges(&recon, stride, 32, 0, 16, 16, 203, P_NONE, 16) {
        DirEdges::Edges { left, .. } => {
            for i in 0..32 {
                assert_eq!(
                    left[i],
                    recon[i * stride + 31],
                    "left[{i}] must be the real reconstructed pixel"
                );
            }
        }
        DirEdges::Flat(_) => panic!("expected edges"),
    }
}

#[test]
fn edges_z3_replicates_when_bottom_left_unavailable() {
    let (recon, stride) = test_frame();
    // 16x16 at (16, 16): mi(4, 4) → has_bl bit 1 of 16 = 0 →
    // replicate left[15].
    match build_directional_edges(&recon, stride, 16, 16, 16, 16, 203, P_NONE, 16) {
        DirEdges::Edges { left, .. } => {
            for i in 0..16 {
                assert_eq!(left[i], recon[(16 + i) * stride + 15]);
            }
            let last = recon[31 * stride + 15];
            for i in 16..32 {
                assert_eq!(left[i], last);
            }
        }
        DirEdges::Flat(_) => panic!("expected edges"),
    }
}

#[test]
fn edges_z2_no_top_fills_above_from_left() {
    let (recon, stride) = test_frame();
    // 16x16 at (16, 0): z2 (135) with no top row: above row filled
    // with left_ref[0]; top_left = left_ref[0].
    match build_directional_edges(&recon, stride, 16, 0, 16, 16, 135, P_NONE, 16) {
        DirEdges::Edges {
            above,
            left,
            top_left,
        } => {
            let lref0 = recon[15];
            // num_top = txwpx (n_topright = -1 for z2).
            for a in above.iter().take(16) {
                assert_eq!(*a, lref0);
            }
            // Beyond num_top: C memset default 127.
            for a in above.iter().skip(16) {
                assert_eq!(*a, 127);
            }
            for (i, l) in left.iter().enumerate().take(16) {
                assert_eq!(*l, recon[i * stride + 15]);
            }
            assert_eq!(top_left, lref0);
        }
        DirEdges::Flat(_) => panic!("expected edges"),
    }
}

#[test]
fn invalid_rdo_shape_does_not_panic() {
    let (recon, stride) = test_frame();
    // 8x64 is not an AV1 block size (RDO transient from 4:1 splits of
    // partial-SB areas). Must fall back to unavailable TR/BL, not panic.
    let e = build_directional_edges(&recon, stride, 8, 64, 8, 64, 45, P_NONE, 16);
    match e {
        DirEdges::Edges { above, .. } => {
            // n_topright = 0 → replication after the 8 real pixels.
            let last = recon[63 * stride + 15];
            for a in above.iter().take(72).skip(8) {
                assert_eq!(*a, last);
            }
        }
        DirEdges::Flat(_) => panic!("expected edges"),
    }
}

/// `dr_predict_hbd` at bd=8 MUST reproduce the C-verified u8 `dr_predict`
/// byte-for-byte on u8-range content (base = 128, hbd kernels clip to 255).
/// This transitively verifies the hbd wrapper's edge-array construction and
/// kernel wiring (the only bd10-specific parts are the base±1 constants —
/// checked against C `build_intra_predictors_high` — and the bd param to the
/// FFI-verified hbd kernels). The bd8 path itself is untouched (additive).
#[test]
fn dr_predict_hbd_bd8_matches_u8_dr_predict() {
    let (recon, stride) = test_frame();
    let recon16: Vec<u16> = recon.iter().map(|&p| p as u16).collect();
    // Directional angles: D45..D203 (modes 3..=8) and V/H (1,2) with a
    // nonzero delta — the cases that route through dr_predict.
    let cases: &[(u8, i8)] = &[
        (3, 0),
        (4, 0),
        (5, 0),
        (6, 0),
        (7, 0),
        (8, 0),
        (3, 2),
        (4, -2),
        (5, 3),
        (6, -3),
        (7, 1),
        (8, -1),
        (1, 1),
        (1, -2),
        (2, 2),
        (2, -3),
    ];
    // Interior, top-edge, left-edge and corner positions × square sizes.
    let blocks: &[(usize, usize, usize, usize)] = &[
        (64, 64, 32, 32),
        (72, 68, 8, 8),
        (80, 80, 16, 16),
        (96, 0, 16, 16),
        (0, 96, 16, 16),
        (0, 0, 8, 8),
        (64, 0, 32, 32),
        (0, 64, 32, 32),
        (16, 48, 16, 16),
    ];
    for &(px, py, txw, txh) in blocks {
        for &(mode, delta) in cases {
            let p_angle = MODE_TO_ANGLE_MAP[mode as usize] + delta as i32 * 3;
            let g = DrGeom {
                px,
                py,
                txw,
                txh,
                mi_row: py >> 2,
                mi_col: px >> 2,
                bw_px: txw,
                bh_px: txh,
                row_off: 0,
                col_off: 0,
                ss: 0,
                frame_w: 128,
                frame_h: 128,
                // 64px superblocks — this bd8-vs-bd10 equivalence test
                // is about the predictor, not the SB geometry.
                sb_mi_size: 16,
                tile: TileMi::whole_frame(128, 128),
            };
            for &edge_filter in &[false, true] {
                for &filt_type in &[0i32, 1] {
                    let mut d8 = alloc::vec![0u8; txw * txh];
                    dr_predict(
                        |x, y| recon[y * stride + x],
                        &g,
                        p_angle,
                        edge_filter,
                        filt_type,
                        P_NONE,
                        &mut d8,
                    );
                    let mut d16 = alloc::vec![0u16; txw * txh];
                    dr_predict_hbd(
                        |x, y| recon16[y * stride + x],
                        &g,
                        p_angle,
                        edge_filter,
                        filt_type,
                        P_NONE,
                        &mut d16,
                        8,
                    );
                    for (i, (&a, &b)) in d8.iter().zip(d16.iter()).enumerate() {
                        assert_eq!(
                            a as u16, b,
                            "mismatch px{px} py{py} {txw}x{txh} mode{mode} d{delta} \
                                 ef{edge_filter} ft{filt_type} idx{i}"
                        );
                    }
                }
            }
        }
    }
}
