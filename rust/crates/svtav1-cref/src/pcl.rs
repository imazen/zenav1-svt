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

unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn ref_pcl_chroma_complexity_check_pred(
        prior_chroma: i32,
        prior_cfl: i32,
        bwidth_uv: i32,
        bheight_uv: i32,
        bsize_uv: i32,
        in_y: *const u8,
        in_y_stride: i32,
        in_u: *const u8,
        in_v: *const u8,
        in_uv_stride: i32,
        pr_y: *const u8,
        pr_y_stride: i32,
        pr_u: *const u8,
        pr_v: *const u8,
        pr_uv_stride: i32,
        use_var: i32,
        cfl_cplx_th: i32,
        out_chroma: *mut i32,
        out_cfl: *mut i32,
    ) -> i32;
}

/// One plane of a block, already offset to the block's first sample.
#[derive(Clone, Copy)]
pub struct RefPlane<'a> {
    pub data: &'a [u8],
    pub stride: usize,
}

/// Reference `chroma_complexity_check_pred` (product_coding_loop.c:6013,
/// exported).
///
/// Returns `(chroma_complexity, cfl_complexity)` as raw `COMPONENT_TYPE`
/// values, or `None` if the shim could not allocate.
///
/// Every plane slice must hold at least `stride * bheight_uv` bytes — the
/// widest extent the C function reads.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn chroma_complexity_check_pred(
    prior_chroma: i32,
    prior_cfl: i32,
    bwidth_uv: usize,
    bheight_uv: usize,
    bsize_uv: usize,
    input: [RefPlane<'_>; 3],
    pred: [RefPlane<'_>; 3],
    use_var: bool,
    cfl_cplx_th: u32,
) -> Option<(i32, i32)> {
    for p in input.iter().chain(pred.iter()) {
        assert!(
            p.data.len() >= p.stride * bheight_uv,
            "the shim copies stride * bheight_uv bytes of every plane"
        );
    }
    let mut out_chroma = 0i32;
    let mut out_cfl = 0i32;
    let rc = unsafe {
        ref_pcl_chroma_complexity_check_pred(
            prior_chroma,
            prior_cfl,
            bwidth_uv as i32,
            bheight_uv as i32,
            bsize_uv as i32,
            input[0].data.as_ptr(),
            input[0].stride as i32,
            input[1].data.as_ptr(),
            input[2].data.as_ptr(),
            input[1].stride as i32,
            pred[0].data.as_ptr(),
            pred[0].stride as i32,
            pred[1].data.as_ptr(),
            pred[2].data.as_ptr(),
            pred[1].stride as i32,
            i32::from(use_var),
            cfl_cplx_th as i32,
            &raw mut out_chroma,
            &raw mut out_cfl,
        )
    };
    (rc == 0).then_some((out_chroma, out_cfl))
}
