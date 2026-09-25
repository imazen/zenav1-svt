use super::*;

fn tile_full_frame(mi_cols: i32, mi_rows: i32) -> TileMiBounds {
    TileMiBounds {
        mi_col_start: 0,
        mi_col_end: mi_cols,
        mi_row_start: 0,
        mi_row_end: mi_rows,
    }
}

#[test]
fn ibc_ctrls_level_table_shape() {
    assert!(!IbcCtrls::for_level(0).enabled);
    let l3 = IbcCtrls::for_level(3);
    assert!(l3.enabled && l3.palette_hint && l3.nsq_parent_gating && l3.mesh_qp_scaling);
    assert_eq!(l3.max_block_size_hash, 64);
    let l7 = IbcCtrls::for_level(7);
    assert_eq!(l7.max_block_size_hash, 8);
    assert_eq!(l7.search_dir, 1);
    assert_eq!(l7.exhaustive_mesh_thresh, u64::MAX);
    assert!(!l7.mesh_qp_scaling); // unassigned at this level, see for_level's doc
}

#[test]
fn allintra_intrabc_level_table() {
    assert_eq!(allintra_intrabc_level(0, true, true), 3);
    assert_eq!(allintra_intrabc_level(4, true, true), 7);
    assert_eq!(allintra_intrabc_level(5, true, true), 0);
    assert_eq!(allintra_intrabc_level(0, false, true), 0);
    assert_eq!(allintra_intrabc_level(0, true, false), 0);
}

#[test]
fn is_chroma_reference_matches_c_parity_rule() {
    // Even-dimensioned luma (bw_mi=bh_mi=2, an 8x8 block): the
    // `!(bh&1)` / `!(bw&1)` disjuncts are unconditionally true, so
    // EVERY mi position is a chroma reference -- mi parity never
    // matters once a block is >=8x8 in both dims under 4:2:0.
    assert!(is_chroma_reference(0, 0, 2, 2, 1, 1));
    assert!(is_chroma_reference(1, 1, 2, 2, 1, 1));
    assert!(is_chroma_reference(0, 1, 2, 2, 1, 1));

    // Odd bw_mi (BLOCK_4X8, bw_mi=1, bh_mi=2): the column clause now
    // genuinely depends on mi_col parity (the row clause stays always
    // true since bh_mi=2 is even) -- only odd mi_col is a reference,
    // matching AV1's "last sub-8-wide column carries chroma" rule.
    assert!(!is_chroma_reference(0, 0, 1, 2, 1, 1));
    assert!(is_chroma_reference(0, 1, 1, 2, 1, 1));

    // Both dims odd (BLOCK_4X4): a reference only when BOTH mi_row and
    // mi_col are odd.
    assert!(!is_chroma_reference(0, 0, 1, 1, 1, 1));
    assert!(!is_chroma_reference(1, 0, 1, 1, 1, 1));
    assert!(!is_chroma_reference(0, 1, 1, 1, 1, 1));
    assert!(is_chroma_reference(1, 1, 1, 1, 1, 1));
}

#[test]
fn find_ref_dv_two_branches() {
    let tile = tile_full_frame(32, 32);
    // Room above (mi_row - mib_size >= tile_row_start): vertical-only.
    let dv_below = find_ref_dv(tile, 16, 20);
    assert_eq!(
        dv_below,
        Mv {
            x: 0,
            y: -16 * 4 * 8
        }
    );
    // No room above: horizontal-only, delayed by INTRABC_DELAY_PIXELS.
    let dv_top = find_ref_dv(tile, 16, 0);
    assert_eq!(
        dv_top,
        Mv {
            x: ((-16 * 4 - INTRABC_DELAY_PIXELS) * 8) as i16,
            y: 0
        }
    );
}

#[test]
fn resolve_dv_ref_coerces_invalid_and_falls_back() {
    let tile = tile_full_frame(32, 32);
    // Both invalid -> falls all the way to find_ref_dv.
    let resolved = resolve_dv_ref(Mv::INVALID, Mv::INVALID, tile, 16, 20);
    assert_eq!(resolved, find_ref_dv(tile, 16, 20));
    // nearestmv nonzero -> used directly.
    let nz = Mv { x: 40, y: -8 };
    assert_eq!(resolve_dv_ref(nz, Mv::ZERO, tile, 16, 20), nz);
    // nearestmv zero, nearmv nonzero -> nearmv used.
    assert_eq!(resolve_dv_ref(Mv::ZERO, nz, tile, 16, 20), nz);
}

#[test]
fn is_dv_valid_rejects_self_reference() {
    // DV = (0,0): "copy from here" is never legal -- the "already
    // coded" wavefront constraint always rejects it (src_sb64 ==
    // active_sb64, never >= active_sb64 - INTRABC_DELAY_SB64 apart).
    let tile = tile_full_frame(64, 64);
    assert!(!is_dv_valid(Mv::ZERO, 20, 20, 8, 8, 2, 2, tile, 4, 64));
}

#[test]
fn is_dv_valid_rejects_out_of_tile() {
    let tile = tile_full_frame(64, 64);
    // DV pointing above the tile's top edge.
    let dv = Mv { x: 0, y: -1000 * 8 };
    assert!(!is_dv_valid(dv, 20, 20, 8, 8, 2, 2, tile, 4, 64));
}

#[test]
fn is_dv_valid_rejects_subpel() {
    let tile = tile_full_frame(64, 64);
    let dv = Mv { x: 5, y: 0 }; // not a multiple of 8
    assert!(!is_dv_valid(dv, 20, 20, 8, 8, 2, 2, tile, 4, 64));
}

#[test]
fn mv_joint_index_table() {
    assert_eq!(mv_joint_index(0, 0), 0);
    assert_eq!(mv_joint_index(5, 0), 1);
    assert_eq!(mv_joint_index(0, 5), 2);
    assert_eq!(mv_joint_index(5, 5), 3);
}

#[test]
fn round_power_of_two_matches_c_macro() {
    assert_eq!(round_power_of_two(10, 2), 3); // (10+2)>>2 = 3
    assert_eq!(round_power_of_two(0, 4), 0);
}

#[test]
fn divide_and_round_half_up() {
    assert_eq!(divide_and_round(10, 4), 3); // (10+2)/4
    assert_eq!(divide_and_round(0, 5), 0);
}

#[test]
fn qp_scaling_disabled_is_identity() {
    assert_eq!(qp_based_th_scaling_factors(false, 40), (1, 1));
    let mut ctrls = IbcCtrls::for_level(3);
    let before = ctrls.mesh_patterns;
    scale_mesh_patterns_by_qp(&mut ctrls, false, 40);
    assert_eq!(ctrls.mesh_patterns, before);
}

#[test]
fn qp_scaling_low_qp_uses_linear_branch() {
    // qp < 46: q_weight = max(10, qp), denom = 63 (MAX_QP_VALUE).
    assert_eq!(qp_based_th_scaling_factors(true, 20), (20, 63));
    assert_eq!(qp_based_th_scaling_factors(true, 5), (10, 63));
}

#[test]
fn hash_search_eligible_gate() {
    assert!(hash_search_eligible(8, 8, 64));
    assert!(!hash_search_eligible(8, 16, 64)); // not square
    assert!(!hash_search_eligible(16, 16, 8)); // too big for level 5-7
    assert!(hash_search_eligible(8, 8, 8));
}

#[test]
fn eval_intrabc_after_palette_gate() {
    assert!(eval_intrabc_after_palette(false, 0)); // palette never ran
    assert!(!eval_intrabc_after_palette(true, 0)); // ran, produced nothing
    assert!(eval_intrabc_after_palette(true, 3));
}

#[test]
fn parent_gate_b4_and_nsq() {
    // PART_N, sq_size=4, gating on, parent tested but didn't use IBC.
    assert!(!parent_gate_allows_intrabc(
        true,
        4,
        true,
        false,
        (true, false),
        (false, false)
    ));
    // Same but parent DID use IBC -> allowed.
    assert!(parent_gate_allows_intrabc(
        true,
        4,
        true,
        false,
        (true, true),
        (false, false)
    ));
    // sq_size != 4 -> b4 gate never fires regardless of parent state.
    assert!(parent_gate_allows_intrabc(
        true,
        8,
        true,
        false,
        (true, false),
        (false, false)
    ));
    // NSQ branch mirrors the same shape via sibling_n0.
    assert!(!parent_gate_allows_intrabc(
        false,
        8,
        false,
        true,
        (false, false),
        (true, false)
    ));
}

#[test]
fn allow_intrabc_frame_requires_all_three() {
    assert!(allow_intrabc_frame(true, true, true));
    assert!(!allow_intrabc_frame(false, true, true));
    assert!(!allow_intrabc_frame(true, false, true));
    assert!(!allow_intrabc_frame(true, true, false));
}

#[test]
fn intrabc_default_cdf_value() {
    // AOM_CDF2(30531) -> icdf = 32768 - 30531 = 2237.
    assert_eq!(INTRABC_DEFAULT_CDF, [2237, 0, 0]);
}

#[test]
fn mv_cost_table_zero_is_zero() {
    let ctx = NmvContext::default();
    let tables = build_nmv_cost_table(&ctx, MvSubpelPrecision::None);
    assert_eq!(tables.comp_cost[0].cost(0), 0);
    assert_eq!(tables.comp_cost[1].cost(0), 0);
    // Cost table lookup clamps symmetrically past MV_MAX (see
    // MvComponentCost's doc PORT-NOTE) rather than panicking.
    assert_eq!(
        tables.comp_cost[0].cost(MV_MAX + 10),
        tables.comp_cost[0].cost(MV_MAX)
    );
    assert_eq!(
        tables.comp_cost[0].cost(-MV_MAX - 10),
        tables.comp_cost[0].cost(-MV_MAX)
    );
}

#[test]
fn init_search_sites_shape() {
    let cfg = init_search_sites(256);
    // 1 origin + MAX_MVSEARCH_STEPS(11) levels * 8 sites = 89.
    assert_eq!(cfg.sites.len(), 89);
    assert_eq!(cfg.searches_per_step, 8);
    assert_eq!(cfg.sites[0].mv_x, 0);
    assert_eq!(cfg.sites[0].mv_y, 0);
    // First real step is +/- MAX_FIRST_STEP.
    assert_eq!(cfg.sites[1].mv_y, -MAX_FIRST_STEP);
}

#[test]
fn set_mv_search_range_narrows_only() {
    // C's guards ONLY narrow: a computed bound of +/-MAX_FULL_PEL_VAL
    // (1023) must NOT widen tighter input limits (+/-1000 stays).
    let mut limits = FullMvLimits {
        col_min: -1000,
        col_max: 1000,
        row_min: -1000,
        row_max: 1000,
    };
    let mv = Mv { x: 0, y: 0 };
    set_mv_search_range(&mut limits, mv);
    assert_eq!(limits.col_min, -1000);
    assert_eq!(limits.col_max, 1000);
    // Narrower input bound stays narrower (intersection, not overwrite).
    let mut tight = FullMvLimits {
        col_min: -5,
        col_max: 5,
        row_min: -5,
        row_max: 5,
    };
    set_mv_search_range(&mut tight, mv);
    assert_eq!(tight.col_min, -5);
    assert_eq!(tight.col_max, 5);
}
