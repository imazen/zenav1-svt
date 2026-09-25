use super::*;

/// wiener_restore flag costs from the default CDF: the instrumented
/// captures show bits_none = 768 and bits_wn - (count << 9) = 320 on
/// every cell.
#[test]
fn restore_costs_match_instrumented_c() {
    assert_eq!(wiener_restore_cost(), [768, 320]);
}

/// RDCOST_DBL against captured values: g64 q40 unit RD —
/// cost_none 26642064.625 (bits 768, sse 207986, rdmult 211804) and
/// cost_wn 26499258.34375 (bits 11072, sse 204789).
#[test]
fn rdcost_dbl_matches_instrumented_c() {
    assert_eq!(rdcost_dbl(211804, 768 >> 4, 207986), 26642064.625);
    assert_eq!(rdcost_dbl(211804, 11072 >> 4, 204789), 26499258.34375);
    // g64 q55: NONE wins at the unit level.
    assert_eq!(rdcost_dbl(1303771, 768 >> 4, 671191), 86034676.53125);
    assert_eq!(rdcost_dbl(1303771, 13120 >> 4, 670249), 87879942.7421875);
}

/// M6 controls: presets 4..=6 -> level 4 (no refinement), <=3 -> level
/// 3 (refinement, one step), >=7 disabled.
#[test]
fn allintra_ctrls_match_c() {
    let c6 = wn_filter_ctrls_allintra(6);
    assert!(c6.enabled && c6.use_chroma && c6.filter_tap_lvl == 2 && !c6.use_refinement);
    let c3 = wn_filter_ctrls_allintra(3);
    assert!(c3.enabled && c3.use_refinement && c3.max_one_refinement_step);
    assert!(!wn_filter_ctrls_allintra(7).enabled);
    assert!(!wn_filter_ctrls_allintra(13).enabled);
}
