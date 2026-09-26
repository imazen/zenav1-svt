use super::*;

/// Tier 4: every expectation is hand-derived from the C source line in
/// its comment. All five functions are `static`/`INLINE` in C.
fn txt() -> TxtControls {
    TxtControls {
        enabled: true,
        group_inter_lt_16x16: 2,
        group_inter_gt_eq_16x16: 3,
        group_intra_lt_16x16: 5,
        group_intra_gt_eq_16x16: 4,
    }
}

fn txs() -> TxsControls {
    TxsControls {
        enabled: true,
        intra_class_max_depth_sq: 2,
        intra_class_max_depth_nsq: 1,
        inter_class_max_depth_sq: 1,
        inter_class_max_depth_nsq: 0,
        depth1_txt_group_offset: 3,
        depth2_txt_group_offset: 4,
    }
}

fn state() -> TxShortcutState {
    TxShortcutState {
        perform_mds1: false,
        is_mds3: false,
        use_tx_shortcuts_mds3: false,
        bypass_tx_th: 0,
        block_has_coeff: true,
        luma_fast_dist: 0,
        qp_index: 100,
    }
}

/// `:4102-4110`, exhaustively over the block sizes C names.
#[test]
fn end_tx_depth_matches_the_c_block_size_list() {
    for (w, h) in [
        (64, 64),
        (32, 32),
        (16, 16),
        (64, 32),
        (32, 64),
        (16, 32),
        (32, 16),
        (16, 8),
        (8, 16),
        (64, 16),
        (16, 64),
        (32, 8),
        (8, 32),
        (16, 4),
        (4, 16),
    ] {
        assert_eq!(get_end_tx_depth(w, h), 2, "{w}x{h}");
    }
    assert_eq!(get_end_tx_depth(8, 8), 1);
    for (w, h) in [(8, 4), (4, 8), (4, 4), (128, 128), (128, 64), (64, 128)] {
        assert_eq!(get_end_tx_depth(w, h), 0, "{w}x{h}");
    }
}

/// `:4294-4298`: four fields, not two. An intra-only table would return
/// the intra value for an inter candidate.
#[test]
fn the_type_group_splits_four_ways() {
    let (t, s) = (txt(), txs());
    // TX_16X16 is index 2 (wide 16, high 16) -> "not small".
    let big = cc::tx_size_from_dims(16, 16);
    // TX_8X8 is index 1 -> small.
    let small = cc::tx_size_from_dims(8, 8);
    assert_eq!(get_tx_type_group(&t, &s, big, 0, false, true), 4);
    assert_eq!(get_tx_type_group(&t, &s, small, 0, false, true), 5);
    assert_eq!(get_tx_type_group(&t, &s, big, 0, false, false), 3);
    assert_eq!(get_tx_type_group(&t, &s, small, 0, false, false), 2);
}

/// `:4290-4292`: "small" is `wide < 16 || high < 16`, so a 32x8 rect is
/// small even though one side is large.
#[test]
fn a_rectangular_transform_is_small_if_either_side_is() {
    let (t, s) = (txt(), txs());
    let rect = cc::tx_size_from_dims(32, 8);
    assert_eq!(get_tx_type_group(&t, &s, rect, 0, false, true), 5);
}

/// `:4301-4305`: the depth offsets subtract and floor at 1, and they
/// apply even to the forced group of 1.
#[test]
fn depth_offsets_subtract_and_floor_at_one() {
    let (t, s) = (txt(), txs());
    let big = cc::tx_size_from_dims(16, 16);
    assert_eq!(get_tx_type_group(&t, &s, big, 1, false, true), 1, "4 - 3");
    assert_eq!(
        get_tx_type_group(&t, &s, big, 2, false, true),
        1,
        "4 - 4 -> 1"
    );
    assert_eq!(get_tx_type_group(&t, &s, big, 0, true, true), 1, "forced");
    assert_eq!(
        get_tx_type_group(&t, &s, big, 1, true, true),
        1,
        "forced + offset"
    );
    // A negative offset RAISES the group — nothing in C forbids it.
    let mut s2 = s;
    s2.depth1_txt_group_offset = -2;
    assert_eq!(get_tx_type_group(&t, &s2, big, 1, false, true), 6);
}

/// `:6705-6708`: the two early arms. `!mds_do_txs` pins BOTH bounds to
/// the candidate's own depth, which the intra funnel never does.
#[test]
fn the_early_arms_pin_both_bounds() {
    let s = state();
    let mut t = txs();
    t.enabled = false;
    assert_eq!(
        get_start_end_tx_depth(
            &t,
            true,
            2,
            false,
            true,
            (16, 16),
            (0, 0),
            (64, 64),
            &s,
            false,
            16
        ),
        (0, 0)
    );
    let t = txs();
    assert_eq!(
        get_start_end_tx_depth(
            &t,
            false,
            2,
            false,
            true,
            (16, 16),
            (0, 0),
            (64, 64),
            &s,
            false,
            16
        ),
        (2, 2),
        "a disabled TXS search keeps the candidate's own depth"
    );
}

/// `:6712-6717`: a block overhanging the aligned frame gets depth 0.
#[test]
fn an_overhanging_block_is_pinned_to_depth_zero() {
    let (t, s) = (txs(), state());
    let inside = get_start_end_tx_depth(
        &t,
        true,
        0,
        false,
        true,
        (16, 16),
        (48, 48),
        (64, 64),
        &s,
        false,
        16,
    );
    assert_eq!(inside, (0, 2));
    let over = get_start_end_tx_depth(
        &t,
        true,
        0,
        false,
        true,
        (16, 16),
        (56, 48),
        (64, 64),
        &s,
        false,
        16,
    );
    assert_eq!(over, (0, 0));
}

/// `:6730-6732`: the class caps differ by mode class AND by shape, and
/// the inter caps are the ones an intra-only port cannot reach.
#[test]
fn the_depth_cap_is_keyed_on_mode_class_and_shape() {
    let (t, s) = (txs(), state());
    let d = |inter, square| {
        get_start_end_tx_depth(
            &t,
            true,
            0,
            inter,
            square,
            (16, 16),
            (0, 0),
            (64, 64),
            &s,
            false,
            16,
        )
        .1
    };
    assert_eq!(d(false, true), 2, "intra sq");
    assert_eq!(d(false, false), 1, "intra nsq");
    assert_eq!(d(true, true), 1, "inter sq");
    assert_eq!(d(true, false), 0, "inter nsq");
}

/// `:6720-6726` and `:6734-6736`: the bypass shortcut zeroes both
/// bounds, and the lossless pin then RAISES them back to 1 because it
/// runs after everything else.
#[test]
fn the_bypass_shortcut_and_the_lossless_pin_compose_in_c_order() {
    let t = txs();
    let s = TxShortcutState {
        perform_mds1: true,
        is_mds3: true,
        bypass_tx_th: 10,
        block_has_coeff: false,
        luma_fast_dist: 1,
        qp_index: 100,
        ..state()
    };
    // 8x8 area 64 * qp 100 = 6400; 1 * 10 = 10 < 6400 -> shortcut.
    assert_eq!(
        get_start_end_tx_depth(
            &t,
            true,
            0,
            false,
            true,
            (8, 8),
            (0, 0),
            (64, 64),
            &s,
            false,
            8
        ),
        (0, 0)
    );
    assert_eq!(
        get_start_end_tx_depth(
            &t,
            true,
            0,
            false,
            true,
            (8, 8),
            (0, 0),
            (64, 64),
            &s,
            true,
            8
        ),
        (1, 1),
        "mimic_only_tx_4x4 runs last and overrides the zeroed bounds"
    );
    // A candidate that DID keep coefficients never takes the shortcut.
    let coded = TxShortcutState {
        block_has_coeff: true,
        ..s
    };
    assert_eq!(
        get_start_end_tx_depth(
            &t,
            true,
            0,
            false,
            true,
            (8, 8),
            (0, 0),
            (64, 64),
            &coded,
            false,
            8
        ),
        (0, 1)
    );
}

/// `:4525-4548`: five independent reasons, each checked alone.
#[test]
fn dct_only_has_five_independent_causes() {
    let tx16 = cc::tx_size_from_dims(16, 16);
    let s = state();
    assert!(
        search_dct_dct_only(false, &s, (16, 16), tx16, false, false),
        "txt off"
    );
    let shortcut = TxShortcutState {
        is_mds3: true,
        use_tx_shortcuts_mds3: true,
        ..s
    };
    assert!(search_dct_dct_only(
        true,
        &shortcut,
        (16, 16),
        tx16,
        false,
        false
    ));
    let bypass = TxShortcutState {
        is_mds3: true,
        perform_mds1: true,
        bypass_tx_th: 10,
        block_has_coeff: false,
        luma_fast_dist: 1,
        ..s
    };
    assert!(search_dct_dct_only(
        true,
        &bypass,
        (16, 16),
        tx16,
        false,
        false
    ));
    // 64x64 is larger than 32 in both dimensions.
    let tx64 = cc::tx_size_from_dims(64, 64);
    assert!(search_dct_dct_only(true, &s, (64, 64), tx64, false, false));
    // The baseline 16x16 intra case is NOT dct-only — the positive
    // control that keeps the four above from passing vacuously.
    assert!(!search_dct_dct_only(true, &s, (16, 16), tx16, false, false));
}

/// `:4530` and `:4532` both require MD_STAGE_3; outside it neither
/// shortcut fires even when its own flag is set.
#[test]
fn the_shortcut_arms_are_mds3_only() {
    let tx16 = cc::tx_size_from_dims(16, 16);
    let armed_but_early = TxShortcutState {
        is_mds3: false,
        use_tx_shortcuts_mds3: true,
        perform_mds1: true,
        bypass_tx_th: 10,
        block_has_coeff: false,
        luma_fast_dist: 1,
        ..state()
    };
    assert!(!search_dct_dct_only(
        true,
        &armed_but_early,
        (16, 16),
        tx16,
        false,
        false
    ));
}

/// `:4555-4573`: which table, and the two free exits.
#[test]
fn txt_rate_source_picks_the_table_by_mode_class() {
    let tx16 = cc::tx_size_from_dims(16, 16);
    assert_eq!(
        txt_rate_source(tx16, 3, false, 6, false),
        TxtRateSource::Intra {
            set: cc::ext_tx_set(tx16, false, false) as usize,
            square_tx_size: cc::TXSIZE_SQR_MAP[tx16],
            intra_dir: 6,
            tx_type: 3,
        }
    );
    assert_eq!(
        txt_rate_source(tx16, 3, true, 6, false),
        TxtRateSource::Inter {
            set: cc::ext_tx_set(tx16, true, false) as usize,
            square_tx_size: cc::TXSIZE_SQR_MAP[tx16],
            tx_type: 3,
        }
    );
    // 64x64 admits DCT_DCT only -> no signalling cost.
    let tx64 = cc::tx_size_from_dims(64, 64);
    assert_eq!(
        txt_rate_source(tx64, 0, false, 0, false),
        TxtRateSource::Free
    );
    assert_eq!(
        txt_rate_source(tx64, 0, true, 0, false),
        TxtRateSource::Free
    );
}

/// The reduced-TX-set frame header collapses more sizes to free, and it
/// does so differently for intra and inter.
#[test]
fn the_reduced_tx_set_changes_which_sizes_are_free() {
    let mut differed = 0usize;
    // Only the real AV1 TX shapes: square, 2:1 and 4:1. `tx_size_from_dims`
    // panics on 4x32 / 32x4, which do not exist as transforms.
    const SHAPES: &[(usize, usize)] = &[
        (4, 4),
        (8, 8),
        (16, 16),
        (32, 32),
        (4, 8),
        (8, 4),
        (8, 16),
        (16, 8),
        (16, 32),
        (32, 16),
        (4, 16),
        (16, 4),
        (8, 32),
        (32, 8),
    ];
    {
        for &(w, h) in SHAPES {
            let tx = cc::tx_size_from_dims(w, h);
            for is_inter in [false, true] {
                let full = txt_rate_source(tx, 1, is_inter, 0, false);
                let reduced = txt_rate_source(tx, 1, is_inter, 0, true);
                if full != reduced {
                    differed += 1;
                }
            }
        }
    }
    assert!(
        differed > 0,
        "the reduced-set flag must reach the source decision somewhere"
    );
}
