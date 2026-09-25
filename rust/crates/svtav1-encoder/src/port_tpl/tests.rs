use super::*;

/// `result_model_store` — synth_blk_size 32 writes one cell at (x>>5,
/// y>>5) on the (w+31)/32 grid.
#[test]
fn result_model_store_32() {
    let mut st = TplStats {
        srcrf_dist: 7,
        recrf_dist: 9,
        srcrf_rate: 3,
        recrf_rate: 5,
        ..Default::default()
    };
    let mut grid = vec![TplStats::default(); 4 * 4];
    // aligned_width 128 -> stride 4; block at (64,32) -> (y>>5)*4+(x>>5)
    // = 1*4+2 = 6
    result_model_store(&mut st, &mut grid, 32, 128, 64, 32, 32);
    assert_eq!(grid[6].srcrf_dist, 7);
    assert_eq!(grid[6].recrf_dist, 9);
    assert!(grid[5].srcrf_dist == 0 && grid[7].srcrf_dist == 0);
}

/// synth 16: a size-32 result is quartered and written to the four 16x16
/// cells it covers; a size-16 result writes one cell.
#[test]
fn result_model_store_16() {
    let mut st = TplStats {
        srcrf_dist: 64,
        recrf_dist: 64,
        srcrf_rate: 32,
        recrf_rate: 16,
        ..Default::default()
    };
    let mut grid = vec![TplStats::default(); 8 * 8];
    // aligned_width 128 -> stride 8; block at (32,16) -> idx (1,2)=10
    result_model_store(&mut st, &mut grid, 16, 128, 32, 16, 32);
    for &idx in &[10usize, 11, 18, 19] {
        assert_eq!(grid[idx].srcrf_dist, 16, "idx {idx}");
        assert_eq!(grid[idx].recrf_rate, 4);
    }
    assert_eq!(grid[9].srcrf_dist, 0);

    let mut st2 = TplStats {
        srcrf_dist: 5,
        recrf_dist: 5,
        srcrf_rate: 5,
        recrf_rate: 5,
        ..Default::default()
    };
    let mut grid2 = vec![TplStats::default(); 8 * 8];
    result_model_store(&mut st2, &mut grid2, 16, 128, 32, 16, 16);
    assert_eq!(grid2[10].srcrf_dist, 5);
    assert_eq!(grid2[11].srcrf_dist, 0);
}

/// synth 8: a size-16 result is quartered over a 2x2 patch of the
/// doubled-resolution grid.
#[test]
fn result_model_store_8() {
    let mut st = TplStats {
        srcrf_dist: 40,
        recrf_dist: 40,
        srcrf_rate: 8,
        recrf_rate: 4,
        ..Default::default()
    };
    // aligned_width 128 -> stride ((128+15)/16)<<1 = 16; block at (16,16)
    // -> idx (2,2) = 34
    let mut grid = vec![TplStats::default(); 16 * 8];
    result_model_store(&mut st, &mut grid, 8, 128, 16, 16, 16);
    for &idx in &[34usize, 35, 50, 51] {
        assert_eq!(grid[idx].srcrf_dist, 10, "idx {idx}");
    }
    assert_eq!(grid[33].srcrf_dist, 0);
}

/// `rate_estimator`: eob+1 plus per-coeff floor(log2(level+1)) + (level>0),
/// shifted by AV1_PROB_COST_SHIFT=9.
#[test]
fn rate_estimator_counts() {
    // Tx16x16 scan — coeff[0] is the DC. Put levels at scan positions
    // 0..2 (raster 0,1,16 for the default scan).
    let mut q = [0i32; 256];
    let scan = crate::entropy::scan_tables::scan(TxSize::Tx16x16 as usize, 0);
    q[scan[0] as usize] = -3; // msb(4)=2, +1 -> 3
    q[scan[1] as usize] = 0; // msb(1)=0, +0 -> 0
    q[scan[2] as usize] = 1; // msb(2)=1, +1 -> 2
    let eob = 3;
    let rate = rate_estimator(&q, eob, TxSize::Tx16x16);
    assert_eq!(rate, (3 + 1 + 3 + 0 + 2) << 9);
}

/// `tpl_qindex` without enable_tpl_qps: quantizer_to_qindex[qp] + offset,
/// clamped to MAXQ.
#[test]
fn tpl_qindex_plain() {
    let base = crate::rate_control::QUANTIZER_TO_QINDEX[30] as i32;
    assert_eq!(
        tpl_qindex(30, 0, false, crate::port_picstruct::SliceType::B, 4, 2),
        base
    );
    // clamps at 255
    assert!(tpl_qindex(63, 200, false, crate::port_picstruct::SliceType::B, 4, 2) <= 255);
}

/// `rdcost_tpl` = ((r*rm + half) >> 9) + (d << 7).
#[test]
fn rdcost_shape() {
    assert_eq!(rdcost_tpl(0, 0, 5), 5 << 7);
    assert_eq!(rdcost_tpl(64, 512, 0), (512 * 64 + 256) >> 9);
}

/// `round_floor` rounds -1..-bsize down to -1 (unlike `>>` on positives).
#[test]
fn round_floor_negatives() {
    assert_eq!(round_floor(0, 16), 0);
    assert_eq!(round_floor(15, 16), 0);
    assert_eq!(round_floor(16, 16), 1);
    assert_eq!(round_floor(-1, 16), -1);
    assert_eq!(round_floor(-16, 16), -1);
    assert_eq!(round_floor(-17, 16), -2);
}

/// `get_overlap_area`: a ref pos offset by (-4,-4) px inside a 16x16 cell
/// spreads over the four quadrants 16/48/48/144.
#[test]
fn overlap_area_quadrants() {
    // ref_pos (12,12) inside grid cells of 16 px at base (0,0).
    let a0 = get_overlap_area(0, 0, 12, 12, 0, 16, 16);
    let a1 = get_overlap_area(0, 16, 12, 12, 1, 16, 16);
    let a2 = get_overlap_area(16, 0, 12, 12, 2, 16, 16);
    let a3 = get_overlap_area(16, 16, 12, 12, 3, 16, 16);
    assert_eq!((a0, a1, a2, a3), (16, 48, 48, 144));
    assert_eq!(a0 + a1 + a2 + a3, 256);
}

/// `tpl_model_update` — a cur-frame cell with mv (0,0) and poc pointing at
/// pics[0] deposits (cur_dep_dist, delta_rate) into exactly one ref cell.
#[test]
fn model_update_propagates() {
    // 64x64, synth 16 -> mi_cols_sr = 16, grid stride 4.
    let mut ref_stats = vec![TplStats::default(); 16];
    let mut cur_stats = vec![TplStats::default(); 16];
    // cur cell (mi_row=4, mi_col=4) -> grid idx 5.
    cur_stats[5] = TplStats {
        ref_frame_poc: 0,
        mv: Mv { x: 0, y: 0 },
        recrf_dist: 1000,
        srcrf_dist: 500,
        recrf_rate: 100,
        srcrf_rate: 50,
        ..Default::default()
    };
    {
        let mut pics = [
            TplSynthPic {
                picture_number: 0,
                aligned_width: 64,
                mi_rows: 16,
                mi_cols: 16,
                stats: &mut ref_stats,
            },
            TplSynthPic {
                picture_number: 8,
                aligned_width: 64,
                mi_rows: 16,
                mi_cols: 16,
                stats: &mut cur_stats,
            },
        ];
        // Outer bsize for synth 16 is BLOCK_16X16; walk only block (4,4).
        tpl_model_update(&mut pics, 1, 4, 4, BlockSize::Block16x16, 2, 16, false);
        // mv (0,0): ref_pos (16,16) sits exactly on grid cell (16,16) ->
        // only quadrant 0 overlaps, area 256 = pix_num.
        let ref_cell = &pics[0].stats[5];
        assert_eq!(ref_cell.mc_dep_dist, 500);
        assert_eq!(ref_cell.mc_dep_rate, 50);
        // neighbours untouched
        assert_eq!(pics[0].stats[4].mc_dep_dist, 0);
        assert_eq!(pics[0].stats[6].mc_dep_dist, 0);
    }
}

/// `generate_r0beta`: zero-dep stats -> mc_dep_cost_base > 0 only if
/// recrf_dist > 0; r0 = recrf/(recrf+dep).
#[test]
fn r0beta_no_dep() {
    // 64x64, synth 16, sb_size 64 -> 1 sb. tpl_stats all recrf=100, dep=0.
    let stats = vec![
        TplStats {
            recrf_dist: 100,
            srcrf_dist: 50,
            recrf_rate: 10,
            srcrf_rate: 5,
            ..Default::default()
        };
        16
    ];
    let mut scale = vec![0.0f64; 16];
    let mut beta = vec![0.0f64; 1];
    let out = generate_r0beta(
        16,
        64,
        64,
        64,
        64,
        64,
        8, // SCALE_NUMERATOR — no superres
        16,
        64,
        &stats,
        &[(0, 0)],
        &mut scale,
        &mut beta,
    );
    assert!(out.tpl_is_valid);
    // dep deltas are all 0 -> r0 = 1.0, beta = r0/rk = 1.0.
    assert_eq!(out.r0, 1.0);
    assert_eq!(beta[0], 1.0);
    // scaling factor = 1.2 + rk/r0 with rk = 1 -> 2.2 per cell.
    assert!((scale[0] - 2.2).abs() < 1e-9);
}

/// `TplXd` — C `init_xd_tpl`: edges in subpel units at (mi*4)*8.
#[test]
fn xd_edges() {
    let xd = TplXd::new(68, 120, BlockSize::Block16x16, 64, 32);
    // mi_row = 8, mi_col = 16; bw=bh=4 mi.
    assert_eq!(xd.to_top, -(8 * 4 * 8));
    assert_eq!(xd.to_bottom, (68 - 4 - 8) * 4 * 8);
    assert_eq!(xd.to_left, -(16 * 4 * 8));
    assert_eq!(xd.to_right, (120 - 4 - 16) * 4 * 8);
    assert_eq!(xd.mi_row, 8);
    assert_eq!(xd.mi_col, 16);
}
