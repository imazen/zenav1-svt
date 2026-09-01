//! FFI bindings for the `product_coding_loop.c` candidate-STAGING oracle
//! (lane `wx-pcl`).
//!
//! Backed by `shims/pcl_shims.c`, which drives the REAL exported C symbols
//! listed in that file's header — evidence tier 1
//! (`docs/WORKING-ON-THIS.md` §4).
//!
//! Kept in its own module (and its own C translation unit) so this lane
//! never shares an editable file with the concurrent MD / inter lanes.

unsafe extern "C" {
    fn ref_pcl_sort_full_cost(
        costs: *const u64,
        num_buffers: u32,
        in_indices: *const u32,
        num_to_sort: u32,
        out_indices: *mut u32,
    ) -> i32;
}

/// Reference `sort_full_cost_based_candidates` (product_coding_loop.c:1438,
/// exported).
///
/// `costs[i]` is buffer `i`'s full cost; `indices` are the buffer indices to
/// sort. Returns the sorted indices, or `None` if the shim could not
/// allocate.
#[must_use]
pub fn sort_full_cost_based_candidates(costs: &[u64], indices: &[u32]) -> Option<Vec<u32>> {
    assert!(
        indices.iter().all(|&i| (i as usize) < costs.len()),
        "every index must address a cost"
    );
    let mut out = indices.to_vec();
    let rc = unsafe {
        ref_pcl_sort_full_cost(
            costs.as_ptr(),
            costs.len() as u32,
            indices.as_ptr(),
            indices.len() as u32,
            out.as_mut_ptr(),
        )
    };
    (rc == 0).then_some(out)
}
