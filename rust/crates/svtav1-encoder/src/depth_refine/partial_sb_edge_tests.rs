use super::*;

/// C `set_blocks_to_test` (enc_dec_process.c:1394-1438) at a frame
/// boundary. This is the rule the PD1 walk was missing entirely: before
/// the partial-SB fix, presets 0..=5 never ran this walk on an incomplete
/// superblock at all (`pipeline.rs`'s `refined` required `full_sb`), so a
/// non-64-aligned frame took a structurally different search than C's.
///
/// ANTI-VACUITY: every assert below is about a `has_rows`/`has_cols` FALSE
/// case. On a 64-aligned frame both are always true and only the two
/// interior asserts are exercised — which is exactly why the aligned gate
/// stays at 1036/1036 while the partial cells moved 72 -> 187 of 216.
#[test]
fn set_blocks_to_test_edge_rules() {
    let nsq_on = NsqCfg::for_arm(crate::sc_detect::ScArm::Allintra, 3, 20); // NSQ SEARCH on (preset <= 3)
    let nsq_off = NsqCfg::off(); // presets 4/5: md_disallow_nsq_search
    // Interior node, NSQ search on: the full N/H/V/H4/V4 list.
    assert_eq!(shapes_at_edge(32, &nsq_on, true, true, true).len(), 5);
    // Interior node, NSQ search off: PART_N only.
    assert_eq!(
        shapes_at_edge(32, &nsq_off, true, true, true),
        &[PartitionType::None]
    );
    // BOTH flags false -> tot_shapes = 0 -> forced SPLIT (:1405-1410).
    assert!(shapes_at_edge(32, &nsq_on, true, false, false).is_empty());
    assert!(shapes_at_edge(32, &nsq_off, true, false, false).is_empty());
    // BOTTOM edge (!has_rows) -> EXACTLY PART_H, PARTITION_NONE EXCLUDED
    // (:1417-1421).
    assert_eq!(
        shapes_at_edge(32, &nsq_on, true, false, true),
        &[PartitionType::Horz]
    );
    // RIGHT edge (!has_cols) -> EXACTLY PART_V.
    assert_eq!(
        shapes_at_edge(32, &nsq_on, true, true, false),
        &[PartitionType::Vert]
    );
    // The edge shape is injected even when the NSQ *search* is disabled:
    // C ANDs `md_disallow_nsq_search` with `!inj_hv_incomp` (:1414). That
    // is what lets presets 4/5 code a boundary rect at all.
    assert_eq!(
        shapes_at_edge(32, &nsq_off, true, false, true),
        &[PartitionType::Horz]
    );
    // NSQ GEOMETRY off (allintra enc_mode > M6) -> no shape is injected and
    // the node force-splits (the presets >= 7 rule, same clause :1405).
    assert!(shapes_at_edge(32, &nsq_on, false, false, true).is_empty());
}

/// C `test_depth`'s `shape_block_cnt--` (product_coding_loop.c:10899-10904).
#[test]
fn shape_block_cnt_drops_out_of_frame_subblocks() {
    // Interior 64x64 in a 128x128 aligned frame: nothing dropped.
    assert_eq!(
        shape_block_cnt_edge(64, PartitionType::Horz, 2, 0, 0, 128, 128, true, true),
        2
    );
    assert_eq!(
        shape_block_cnt_edge(64, PartitionType::Horz4, 4, 0, 0, 128, 128, true, true),
        4
    );
    // A single-edge node codes ONLY the first rect (the in-frame half).
    assert_eq!(
        shape_block_cnt_edge(64, PartitionType::Horz, 2, 0, 64, 128, 88, false, true),
        1
    );
    assert_eq!(
        shape_block_cnt_edge(64, PartitionType::Vert, 2, 64, 0, 72, 128, true, false),
        1
    );
    // H4 at y = 0 in a 48-tall aligned frame: has_rows is TRUE (0+32 < 48),
    // yet the 4th quarter starts at y = 48 == aligned_h, so it is dropped.
    assert_eq!(
        shape_block_cnt_edge(64, PartitionType::Horz4, 4, 0, 0, 64, 48, true, true),
        3
    );
    // The V4 mirror on a 48-wide aligned frame.
    assert_eq!(
        shape_block_cnt_edge(64, PartitionType::Vert4, 4, 0, 0, 48, 64, true, true),
        3
    );
}

/// C `svt_aom_partition_rate_cost`'s boundary arms (rd_cost.c:1846-1866).
/// The port's `PartRates` only ever indexed the full 10-symbol alphabet; at
/// a boundary node C prices the BINARY split-vs-{H,V} alphabet instead, and
/// BOTH entries are live (entry 0 is `test_depth`'s `part_rate` for the
/// injected rect, and `update_skip_nsq_based_on_split_rate` reads it too).
#[test]
fn partition_rate_uses_the_boundary_alphabet() {
    let fc = crate::entropy::context::FrameContext::new_default();
    let r = PartRates::from_fc(&fc);
    // 32x32 -> bsl 2 -> ctx row 8 (left = above = 0).
    let row = 8usize;
    let full_split = r.bits(row, PartitionType::Split);
    let full_horz = r.bits(row, PartitionType::Horz);
    // Interior: unchanged, the full alphabet.
    assert_eq!(
        r.bits_edge(row, PartitionType::Split, true, true),
        full_split
    );
    // Both false: C returns 0 — the node codes no partition symbol.
    assert_eq!(r.bits_edge(row, PartitionType::Split, false, false), 0);
    assert_eq!(r.bits_edge(row, PartitionType::Horz, false, false), 0);
    // Bottom edge: the vert_alike binary pair. SPLIT and the rect must
    // differ from each other and from the full-alphabet costs, else the
    // whole boundary-rate distinction would be vacuous.
    let b_split = r.bits_edge(row, PartitionType::Split, false, true);
    let b_rect = r.bits_edge(row, PartitionType::Horz, false, true);
    assert_ne!(b_split, b_rect);
    assert_ne!(b_split, full_split);
    assert_ne!(b_rect, full_horz);
    // Right edge uses the OTHER gather, and the two tables are genuinely
    // different (C's horz/vert alike gathers are asymmetric).
    let r_split = r.bits_edge(row, PartitionType::Split, true, false);
    let r_rect = r.bits_edge(row, PartitionType::Vert, true, false);
    assert_ne!(r_split, b_split);
    assert_ne!(r_rect, b_rect);
    // The boundary table is keyed on `p == PARTITION_SPLIT` ONLY, so every
    // non-SPLIT symbol collapses onto the same rect entry.
    assert_eq!(r.bits_edge(row, PartitionType::None, true, false), r_rect);
    assert_eq!(r.bits_edge(row, PartitionType::Horz4, true, false), r_rect);
}

/// C's `tested_blk[PART_N][0]` guard on every PD1 refinement gate
/// (enc_dec_process.c:1550, 1566, 1586, 1634, 1698-1711, 1859, 1868),
/// spelled out in C's own comment at :1547 — "For incomplete blocks, H/V
/// partitions may be allowed, while square is not ... check that the SQ
/// block is available before using the cost."
///
/// A PD0 leaf that only ever costed its boundary rect must therefore get
/// `s_depth = e_depth = 0`: it is coded at its own depth, never refined.
/// Without the `Pd0Eval::sq_tested` distinction the rect cost was fed to
/// the deviation gates as if it were a square PART_N cost.
#[test]
fn boundary_pd0_leaf_is_never_refined() {
    let tables = crate::pd0::build_m6_pd0_tables(160);
    let ctrls = DrCtrls::for_preset(4); // ADAPTIVE, s1 = e1 = 15
    let kid16 = || crate::pd0::Pd0Eval {
        root_det: None,
        sq: 16,
        tested: true,
        sq_tested: true,
        cost: 5_000_000,
        split: false,
        off: false,
        children: None,
    };
    // A PD0 leaf at 32x32 whose four visited 16x16 children came in far
    // cheaper — `is_child_to_current_deviation_small` admits the child
    // depth (e = 1) whenever the SQ cost is available.
    let leaf32 = |sq_tested: bool| crate::pd0::Pd0Eval {
        root_det: None,
        sq: 32,
        tested: true,
        sq_tested,
        cost: 100_000_000,
        split: false,
        off: false,
        children: Some(alloc::boxed::Box::new([kid16(), kid16(), kid16(), kid16()])),
    };
    let mk = |sq_tested: bool| crate::pd0::Pd0Eval {
        root_det: None,
        sq: 64,
        tested: true,
        sq_tested: true,
        cost: 400_000_000,
        split: true,
        off: false,
        children: Some(alloc::boxed::Box::new([
            leaf32(sq_tested),
            leaf32(true),
            leaf32(true),
            leaf32(true),
        ])),
    };
    let with_sq = build_refined_scan(&mk(true), &ctrls, 25650, &tables);
    let boundary = build_refined_scan(&mk(false), &ctrls, 25650, &tables);
    let q0 = |s: &RefScan| {
        let c = s.children.as_ref().expect("root splits");
        (c[0].split_flag, c[0].children.is_some())
    };
    assert_eq!(
        q0(&with_sq),
        (true, true),
        "a square-costed PD0 leaf admits the child depth"
    );
    assert_eq!(
        q0(&boundary),
        (false, false),
        "a boundary PD0 leaf has no SQ cost: C skips both deviation gates"
    );
}
