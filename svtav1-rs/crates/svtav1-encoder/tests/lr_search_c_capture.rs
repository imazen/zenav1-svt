//! Pinned instrumented-C captures for the Wiener loop-restoration search.
//!
//! `tests/data/c_dgd_g64_q{20,40}_p6.bin` are the EXACT post-CDEF luma
//! recons the C encoder's restoration search consumed at the gradient-64
//! preset-6 identity cells (dumped by the SVT_LRDBG-instrumented scratch
//! build whose OBUs were verified byte-identical to the baseline encoder;
//! docs/captures/gradient_*_p6.lrdbg.txt hold the matching dumps). Running
//! our `search_restoration_still` on them must reproduce C's solved taps,
//! per-unit picks and frame types bit-exactly — this pins the whole chain
//! (find_average -> compute_stats -> wiener_decompose_sep_sym ->
//! finalize_sym_filter -> compute_score -> try_restoration_unit SSE ->
//! count_wiener_bits/RDCOST_DBL finish) against the reference.
//!
//! The production pipeline currently feeds the search OUR recon, which
//! still differs from C's at these cells (unported M6 leaf funnel:
//! filter-intra RDO + MDS3 leaf compare picks), so the taps in OUR streams
//! legitimately differ until that subsystem lands — these fixtures prove
//! the search itself is not the divergence.

use svtav1_encoder::restoration::{search_restoration_still, wn_filter_ctrls_allintra};

/// identity_run's gradient content: y[r][c] = (r*255/h) ^ ((c*3) & 0x3f).
fn gradient_luma(w: usize, h: usize) -> Vec<u8> {
    let mut y = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            y[r * w + c] = (((r * 255) / h) as u8) ^ (((c * 3) & 0x3f) as u8);
        }
    }
    y
}

fn run_cell(dgd: &[u8], rdmult: i64) -> svtav1_encoder::restoration::FrameRestInfo {
    let w = 64usize;
    let h = 64usize;
    let src_y = gradient_luma(w, h);
    let flat = vec![128u8; (w / 2) * (h / 2)];
    let ctrls = wn_filter_ctrls_allintra(6);
    search_restoration_still(
        &ctrls, &src_y, &flat, &flat, dgd, &flat, &flat, w, h, true, rdmult,
    )
}

/// g64 q40 p6 (qindex 160, rdmult 211804): C solved luma
/// v=[0,-1,0,2,0,-1,0] h=[0,-8,7,2,7,-8,0], frame WIENER; chroma flat ->
/// ill-posed solve keeps the default taps, score 0, per-unit + frame NONE.
#[test]
fn g64_q40_p6_matches_instrumented_c() {
    let dgd = include_bytes!("data/c_dgd_g64_q40_p6.bin");
    let info = run_cell(dgd, 211804);
    assert_eq!(info.planes[0].frame_rtype, 1, "luma frame type = WIENER");
    assert_eq!(info.planes[0].units[0].rtype, 1);
    assert_eq!(
        info.planes[0].units[0].wiener.vfilter,
        [0, -1, 0, 2, 0, -1, 0, 0]
    );
    assert_eq!(
        info.planes[0].units[0].wiener.hfilter,
        [0, -8, 7, 2, 7, -8, 0, 0]
    );
    assert_eq!(info.planes[1].frame_rtype, 0, "flat chroma -> NONE");
    assert_eq!(info.planes[2].frame_rtype, 0);
    // The chroma solve is the ill-posed (linsolve fails on zero stats)
    // path: taps stay at the 5-tap default shape (LRWNSOLVE capture:
    // v=[0,-7,15,-16,15,-7,0]).
    assert_eq!(
        info.planes[1].units[0].wiener.vfilter,
        [0, -7, 15, -16, 15, -7, 0, 0]
    );
}

/// g64 q20 p6 (qindex 80, rdmult 21888): C solved luma
/// v=[0,-1,-2,6,-2,-1,0] h=[0,-3,-2,10,-2,-3,0], frame WIENER.
#[test]
fn g64_q20_p6_matches_instrumented_c() {
    let dgd = include_bytes!("data/c_dgd_g64_q20_p6.bin");
    let info = run_cell(dgd, 21888);
    assert_eq!(info.planes[0].frame_rtype, 1);
    assert_eq!(info.planes[0].units[0].rtype, 1);
    assert_eq!(
        info.planes[0].units[0].wiener.vfilter,
        [0, -1, -2, 6, -2, -1, 0, 0]
    );
    assert_eq!(
        info.planes[0].units[0].wiener.hfilter,
        [0, -3, -2, 10, -2, -3, 0, 0]
    );
    assert_eq!(info.planes[1].frame_rtype, 0);
    assert_eq!(info.planes[2].frame_rtype, 0);
}
