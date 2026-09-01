//! Differential parity: the MD-stage candidate SORT
//! (`svtav1-encoder/src/port_md/nic_prune.rs`) vs the REAL exported C.
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4):
//!
//! | oracle | C |
//! |---|---|
//! | `sort_full_cost_based_candidates` | product_coding_loop.c:1438 |
//!
//! That symbol carries no `svt_aom_` prefix and IS exported (`nm -g`), and
//! it has no prototype in any header — `shims/pcl_shims.c` declares it.
//!
//! The other five functions in `nic_prune` (`sort_fast_cost_based_
//! candidates`, `construct_best_sorted_arrays_md_stage_3` and the three
//! `post_mdsN_nic_pruning`) are `static` in C with no symbol, so they are
//! tier 4 and their vectors live in the module's own `#[cfg(test)]` block,
//! labelled as such.
//!
//! What this pins in particular: **ties.** C's exchange sort is not a
//! stable sort, and the deliberately tie-dense grids below are the whole
//! reason this file exists — a `sort_by_key` port passes every
//! distinct-cost case and fails these.

use svtav1_cref::pcl as cpcl;
use svtav1_encoder::port_md::nic_prune as rnic;

/// A deterministic LCG so a failure names a reproducible seed rather than
/// "some random input".
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1);
        self.0 >> 33
    }
}

fn check(costs: &[u64], indices: &[u32]) {
    let c = cpcl::sort_full_cost_based_candidates(costs, indices)
        .expect("shim allocation must succeed");
    let mut r = indices.to_vec();
    rnic::sort_full_cost_based_candidates(&mut r, |i| costs[i as usize]);
    assert_eq!(r, c, "costs = {costs:?}, indices = {indices:?}");
}

#[test]
fn distinct_costs_match_c() {
    let mut rng = Lcg(0x5eed_1234);
    for n in 1usize..=12 {
        for _ in 0..40 {
            let costs: Vec<u64> = (0..n).map(|_| rng.next()).collect();
            let indices: Vec<u32> = (0..n as u32).collect();
            check(&costs, &indices);
        }
    }
}

/// The case a stable sort gets wrong. Costs are drawn from a range far
/// smaller than the count, so most inputs contain several exact ties.
#[test]
fn tie_dense_costs_match_c() {
    let mut rng = Lcg(0xfeed_beef);
    let mut ties_seen = 0usize;
    for n in 2usize..=10 {
        for _ in 0..300 {
            let costs: Vec<u64> = (0..n).map(|_| rng.next() % 3).collect();
            if costs.windows(2).any(|w| w[0] == w[1]) {
                ties_seen += 1;
            }
            let indices: Vec<u32> = (0..n as u32).collect();
            check(&costs, &indices);
        }
    }
    // Positive control (WORKING-ON-THIS §5): prove the tie grid really
    // contains ties rather than trusting a silent pass.
    assert!(ties_seen > 1000, "tie grid produced only {ties_seen} ties");
}

/// All-equal costs: C's strict `<` never swaps, so the input order must
/// survive verbatim. A comparator with `<=` would reverse it.
#[test]
fn all_equal_costs_keep_the_input_order() {
    for n in 1usize..=8 {
        let costs = vec![7u64; n];
        let indices: Vec<u32> = (0..n as u32).rev().collect();
        let c = cpcl::sort_full_cost_based_candidates(&costs, &indices).unwrap();
        assert_eq!(c, indices, "C must not permute an all-tie list");
        check(&costs, &indices);
    }
}

/// C sorts the buffer indices it is handed, which in the driver are a
/// contiguous run starting at that class's `buffer_start_idx` — not
/// `0..n`. The port must sort the same way for a non-zero start and for a
/// scrambled input permutation.
#[test]
fn non_contiguous_and_permuted_index_lists_match_c() {
    let mut rng = Lcg(0xabcd_ef01);
    let costs: Vec<u64> = (0..24).map(|_| rng.next() % 5).collect();
    for start in [0u32, 1, 7, 16] {
        for len in 1usize..=8 {
            if start as usize + len > costs.len() {
                continue;
            }
            let indices: Vec<u32> = (0..len as u32).map(|k| start + k).collect();
            check(&costs, &indices);
            let mut scrambled = indices.clone();
            scrambled.reverse();
            check(&costs, &scrambled);
        }
    }
}

/// Costs near `u64::MAX` — C's `(uint64_t)~0` "unset buffer" sentinel is a
/// real value the sort sees when a class's pool is not full.
#[test]
fn saturated_costs_match_c() {
    let costs = [u64::MAX, 5, u64::MAX, 0, u64::MAX - 1, 5];
    for len in 1usize..=costs.len() {
        let indices: Vec<u32> = (0..len as u32).collect();
        check(&costs, &indices);
    }
}
