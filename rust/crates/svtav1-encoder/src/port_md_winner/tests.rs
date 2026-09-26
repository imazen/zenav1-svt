use super::*;
use svtav1_types::motion::{CandidateMv, Mv};

fn cand(full: u64, ssim: u64, singleref: bool) -> CandidateCost {
    CandidateCost {
        full_cost: full,
        full_cost_ssim: ssim,
        is_inter_singleref: singleref,
    }
}

/// TIER 4 (mode_decision.c:3820). With one MDS3 candidate the loop does
/// not run, so `order[0]` wins whatever the costs say. A port that
/// "helpfully" scanned anyway would pick a different block here.
#[test]
fn single_candidate_short_circuits_the_scan() {
    let c = [cand(900, 900, false), cand(1, 1, false)];
    assert_eq!(select_winner(&c, &[0, 1], 1, false, 4, false, false, 30), 0);
    // With two, the scan runs and the cheap one wins.
    assert_eq!(select_winner(&c, &[0, 1], 2, false, 4, false, false, 30), 1);
}

/// TIER 4. The SSD comparison is STRICT, so a tie goes to the candidate
/// EARLIER in `best_candidate_index_array` — and the answer is an index
/// into the candidate array taken from that ordering, not the loop
/// position.
#[test]
fn ssd_ties_go_to_the_earlier_entry_in_the_order() {
    let c = [
        cand(500, 0, false),
        cand(500, 0, false),
        cand(500, 0, false),
    ];
    assert_eq!(
        select_winner(&c, &[2, 0, 1], 3, false, 4, false, false, 30),
        2
    );
    assert_eq!(
        select_winner(&c, &[1, 2, 0], 3, false, 4, false, false, 30),
        1
    );
}

/// TIER 4 (mode_decision.c:3863-3866). The unipred bias applies only
/// when ALL THREE of `unipred_bias`, `is_noise_level` and
/// `is_inter_singleref_mode` hold, and it makes the candidate CHEAPER.
#[test]
fn unipred_bias_needs_all_three_conditions() {
    // qp 10 -> bias 85. 1000 * 85 / 100 = 850 < 900.
    let c = [cand(900, 0, false), cand(1000, 0, true)];
    assert_eq!(select_winner(&c, &[0, 1], 2, false, 4, true, true, 10), 1);
    // Any one condition off and the raw 1000 loses.
    assert_eq!(select_winner(&c, &[0, 1], 2, false, 4, false, true, 10), 0);
    assert_eq!(select_winner(&c, &[0, 1], 2, false, 4, true, false, 10), 0);
    let c2 = [cand(900, 0, false), cand(1000, 0, false)];
    assert_eq!(select_winner(&c2, &[0, 1], 2, false, 4, true, true, 10), 0);
    // qp 48 -> bias 100, i.e. no discount at all.
    assert_eq!(select_winner(&c, &[0, 1], 2, false, 4, true, true, 48), 0);
}

/// TIER 4 (mode_decision.c:3835-3857). Pass two takes the lowest SSIM
/// among candidates whose SSD is within `factor * lowest_ssd`; a
/// candidate with a better SSIM but an SSD outside the slack does NOT
/// win. The slack is 2 % at 1080p and above, 3 % below — so the same
/// cell can flip on resolution alone.
#[test]
fn ssim_pass_two_respects_the_ssd_slack_and_the_resolution() {
    // lowest ssd 1000 -> threshold 1020 at >=1080p, 1030 below.
    let c = [cand(1000, 900, false), cand(1025, 100, false)];
    assert_eq!(select_winner(&c, &[0, 1], 2, true, 4, false, false, 30), 0);
    assert_eq!(select_winner(&c, &[0, 1], 2, true, 3, false, false, 30), 1);
}

/// TIER 4. On EQUAL ssim the lower SSD wins, and that tie-break is NOT
/// gated on the threshold — C's `else if` branch checks only the SSD.
#[test]
fn equal_ssim_falls_back_to_ssd_without_the_threshold_gate() {
    // Both have ssim 100. The first sets ssim_lowest; the second is far
    // outside the slack (1000 * 1.02 = 1020) yet still wins on SSD?  No:
    // its SSD is HIGHER, so it does not. Reverse the order and the
    // cheaper-SSD one wins through the else-if.
    let c = [cand(1010, 100, false), cand(1000, 100, false)];
    assert_eq!(select_winner(&c, &[0, 1], 2, true, 4, false, false, 30), 1);
    assert_eq!(select_winner(&c, &[1, 0], 2, true, 4, false, false, 30), 1);
}

fn stack(weights: [i32; 8]) -> Vec<CandidateMv> {
    weights
        .iter()
        .map(|&w| CandidateMv {
            this_mv: Mv { x: 0, y: 0 },
            comp_mv: Mv { x: 0, y: 0 },
            weight: w,
        })
        .collect()
}

fn inputs<'a>(mode: u8, s: &'a [CandidateMv]) -> WinnerInputs<'a> {
    WinnerInputs {
        mode,
        is_inter_block: mode >= NEARESTMV,
        is_intra_mode: mode < NEARESTMV,
        use_intrabc: false,
        skip_mode_allowed: false,
        cand_skip_mode: false,
        cand_block_has_coeff: 1,
        total_rate: 100,
        full_dist: 200,
        pd_pass_1: true,
        fixed_partition: false,
        full_lambda: 1000,
        ref_mv_count: 4,
        ref_mv_stack: s,
        palette: None,
        allow_palette: true,
    }
}

/// TIER 4 (mode_decision.c:3909-3931). The drl contexts are `-1` where
/// the ref-MV stack is too short — a sentinel, not a zero — and only a
/// NEWMV-family mode fills `drl_ctx` while only a NEARMV-family mode
/// fills `drl_ctx_near`.
#[test]
fn drl_contexts_use_minus_one_for_a_short_stack() {
    let s = stack([700, 700, 700, 700, 0, 0, 0, 0]);
    // NEWMV with ref_mv_count 4: idx 0 and 1 both satisfy count > idx+1.
    let mut i = inputs(crate::port_entropy_inter::modes::NEWMV, &s);
    let out = winner_signals(&i);
    assert_eq!(out.drl_ctx, [0, 0]);
    assert_eq!(out.drl_ctx_near, [0, 0]);
    // count 2: only idx 0 qualifies, idx 1 becomes -1.
    i.ref_mv_count = 2;
    assert_eq!(winner_signals(&i).drl_ctx, [0, -1]);
    // NEARMV fills the _near array from idx 1..3 instead.
    let mut n = inputs(14 /* NEARMV */, &s);
    n.ref_mv_count = 3;
    let out = winner_signals(&n);
    assert_eq!(out.drl_ctx, [0, 0], "NEARMV does not fill drl_ctx");
    assert_eq!(out.drl_ctx_near, [0, -1]);
}

/// TIER 4 (mode_decision.c:3951-3969). `skip_mode` is OR-ed with
/// "no coefficients" only when the candidate allows it, and a
/// skip-mode block then reports NO coefficients — so `skip` follows.
#[test]
fn skip_mode_folds_into_the_coefficient_state() {
    let s = stack([0; 8]);
    let mut i = inputs(crate::port_entropy_inter::modes::NEWMV, &s);

    // Coefficients present, skip-mode not allowed: nothing folds.
    let out = winner_signals(&i);
    assert!(out.block_has_coeff && !out.skip_mode && !out.skip);

    // No coefficients and skip-mode allowed: skip_mode turns on.
    i.cand_block_has_coeff = 0;
    i.skip_mode_allowed = true;
    let out = winner_signals(&i);
    assert!(!out.block_has_coeff && out.skip_mode && out.skip);

    // Coefficients present AND skip-mode allowed: the OR does not fire,
    // so the block keeps its coefficients.
    i.cand_block_has_coeff = 1;
    let out = winner_signals(&i);
    assert!(out.block_has_coeff && !out.skip_mode && !out.skip);
}

/// TIER 4 (mode_decision.c:3936-3948). An intra winner that is NOT
/// IntraBC clears `skip_mode_allowed` on the candidate — which then
/// suppresses the fold above. An IntraBC winner does not.
#[test]
fn an_intra_winner_clears_skip_mode_allowed_unless_it_is_intrabc() {
    let s = stack([0; 8]);
    let mut i = inputs(0 /* DC_PRED */, &s);
    i.skip_mode_allowed = true;
    i.cand_block_has_coeff = 0;
    let out = winner_signals(&i);
    assert!(!out.skip_mode_allowed);
    assert!(!out.skip_mode, "the fold must not fire once it is cleared");

    i.use_intrabc = true;
    i.is_inter_block = true; // is_inter_block counts IntraBC
    let out = winner_signals(&i);
    assert!(out.skip_mode_allowed && out.skip_mode);
}

/// TIER 4. `blk_ptr->cost` is NOT written when
/// `pd_pass == PD_PASS_1 && fixed_partition` — at inter-depth decision
/// the SB lambda is used instead of the block's tuned one.
#[test]
fn cost_is_skipped_under_fixed_partition_at_pass_1() {
    let s = stack([0; 8]);
    let mut i = inputs(crate::port_entropy_inter::modes::NEWMV, &s);
    assert!(winner_signals(&i).cost.is_some());
    i.fixed_partition = true;
    assert!(winner_signals(&i).cost.is_none());
    i.pd_pass_1 = false;
    assert!(winner_signals(&i).cost.is_some());
}

/// TIER 4 (mode_decision.c:3682-3752). Light PD1 zeroes the palette
/// unconditionally, stamps the drl contexts with no `PD_PASS_1` gate,
/// clears `skip_mode_allowed` for ANY intra mode (no IntraBC exception),
/// and never computes a cost.
#[test]
fn light_pd1_differs_from_the_full_form_in_three_places() {
    let s = stack([700, 0, 0, 0, 0, 0, 0, 0]);
    let mut i = inputs(crate::port_entropy_inter::modes::NEWMV, &s);
    i.pd_pass_1 = false;
    i.palette = Some([4, 4]);
    i.ref_mv_count = 4;

    let light = winner_signals_light_pd1(&i);
    assert_eq!(light.cost, None);
    assert_eq!(light.palette_size, [0, 0]);
    // Stamped even though pd_pass_1 is false.
    assert_eq!(light.drl_ctx, [1, 2]);

    // The full form at pass 0 stamps nothing.
    assert_eq!(winner_signals(&i).drl_ctx, [0, 0]);

    // Intra + IntraBC: the full form KEEPS skip_mode_allowed, light PD1
    // clears it.
    let mut ibc = inputs(0, &s);
    ibc.skip_mode_allowed = true;
    ibc.use_intrabc = true;
    ibc.is_inter_block = true;
    assert!(winner_signals(&ibc).skip_mode_allowed);
    assert!(!winner_signals_light_pd1(&ibc).skip_mode_allowed);
}
