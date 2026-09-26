use super::*;
use crate::inter_me::context::{MeB64Output, MeCandidate};
use crate::inter_me_arm::FrameMe;
use crate::picture::{PaddedPlane, PaddedRef};
use crate::port_md::drl::DRL_MODE_CONTEXTS;
use crate::port_md::md_search::PmeExit;

const W: usize = 128;
const H: usize = 128;
/// The true displacement, in full pels.
///
/// **2, not the campaign's 3, and that is load-bearing.** C's PME
/// full-pel window here is `+-(full_pel_search_width >> 1)` after the qp
/// modulation — `MAX(3, ROUND(7 * 40 / 63)) = 4`, so `+-2` FULL PELS
/// around the MVP. At a 3-pel displacement the full-pel search cannot
/// reach the truth at all and only the sub-pel stage's two-iteration
/// overshoot gets there, which makes the landing point a property of the
/// interpolator on this fixture rather than of the search. At 2 the
/// truth is inside the window and the assertion is about the search.
const SHIFT: usize = 2;

/// A TRIANGLE wave in `2x + y`, so a wrong MV costs real variance in
/// BOTH components (a pure horizontal ramp is invariant to vertical
/// error and would let a y-axis defect pass).
///
/// **Triangle, not `% 256`.** A sawtooth has a one-pixel cliff every
/// 256 samples, and the sub-pel search interpolates ACROSS it: the
/// first version of this fixture used `(2x + y) % 256` and the
/// refinement walked off the true `-3` full-pel match to `-2.25`,
/// because the interpolated cliff scored better than the exact match.
/// That is a property of the fixture, not of the port — pick content
/// the interpolator cannot beat the truth on.
fn source() -> alloc::vec::Vec<u8> {
    (0..W * H)
        .map(|i| {
            let (x, y) = (i % W, i / W);
            let t = (x * 2 + y) % 510;
            let base = if t < 255 { t } else { 509 - t };
            // A deterministic per-pixel DITHER on top. Without it the
            // triangle wave is locally linear, the interpolator
            // reproduces it EXACTLY at every sub-pel offset, and the
            // search is left minimising MV RATE alone — which walks off
            // the true match to a cheaper MV and makes the assertion
            // below a statement about the cost table rather than about
            // the search. The dither shifts WITH the content (it is a
            // function of the source position), so the true full-pel
            // match still scores zero variance and everything else does
            // not.
            let d = (x.wrapping_mul(37) ^ y.wrapping_mul(101)) % 19;
            (base.saturating_add(d).min(255)) as u8
        })
        .collect()
}

/// The reference is the source shifted LEFT by [`SHIFT`] pixels, so the
/// exact match sits at full-pel MV `x = -SHIFT`.
///
/// The sign is the whole point and it is easy to get backwards: the
/// prediction reads `ref[x + mv]`, so a NEGATIVE mv must find the
/// source's content at a SMALLER reference x — i.e. the reference is the
/// source moved toward x = 0. Building it the other way makes `-SHIFT`
/// the WORST full-pel offset, and then a search that walks away from it
/// is behaving correctly while the assertion fails.
fn reference() -> alloc::vec::Vec<u8> {
    let src = source();
    let mut r = alloc::vec![0u8; W * H];
    for y in 0..H {
        for x in 0..W {
            r[y * W + x] = src[y * W + (x + SHIFT).min(W - 1)];
        }
    }
    r
}

fn zero_cost() -> MvCostTable {
    MvCostTable::zeroed()
}

/// One b64 whose ONLY ME candidate is LIST 1's — C's measured shape on
/// `gradient 128x128 q40 p8` (`SVT_HME_OUT`: `n=1 c=[0:dir=1 ...]`,
/// `mv0=(0,0) mvl1=(-3,0)`).
fn frame_me_list1_only() -> FrameMe {
    let (max_cand, max_refs, max_l0) = (13usize, 5usize, 3usize);
    let mut b = MeB64Output::new(max_cand, max_refs);
    // Every 64x64/32x32/... pu of the b64 gets the same single list-1
    // candidate, so the driver reaches the same state at any block size.
    for pu in 0..b.total_me_candidate_index.len() {
        b.total_me_candidate_index[pu] = 1;
        b.me_candidate_array[pu * max_cand] = MeCandidate::new(1, 0, 0, 0, 1);
        // list 0's slot is left at (0,0) — C never writes it when the
        // list-0 search is pruned, and reading it as if it were a result
        // is the defect §1z¹³ measured.
        b.me_mv_array[pu * max_refs + max_l0] = Mv {
            x: -(SHIFT as i16),
            y: 0,
        };
    }
    FrameMe {
        per_b64: alloc::vec![b; 4],
        b64_cols: 2,
        b64_rows: 2,
        max_refs,
        max_cand,
        max_l0,
        enable_me_8x8: true,
        enable_me_16x16: true,
    }
}

fn cfg() -> SearchFrameCfg {
    frame_cfg(&SearchFrameInputs {
        // C's resolved levels on the campaign's `p8` video cells,
        // read back off C's own context through `SVT_INJCFG_OUT`'s
        // `PMEST` line and matched to the control tables:
        //   `pme=1/0/7/5/MIN/25/16/50/32/1/1`  -> md_pme level 4
        //   `sme=1/2/0/1/2/0/0/MAX/0/104/4`    -> me_subpel level 4
        //   `spme=1/3/0/1/2/0/0/MAX/0/104/0`   -> pme_subpel level 2
        md_pme_level: 4,
        me_subpel_level: 4,
        pme_subpel_level: 2,
        md_nsq_mv_search_level: 2,
        // p8 -> `interpolation_search_level` 4 (`IFS_MDS3`).
        interpolation_search_level: 4,
        dist_based_ref_pruning: 0,
        cli_qp: 40,
        picture_qp: 160,
        pme_qp_based_th_scaling: true,
        base_q_idx: 160,
        allow_high_precision_mv: false,
        approx_inter_rate: 0,
        pic_width: W as u32,
        pic_height: H as u32,
    })
    .expect("the campaign's levels are in-domain for every C control table")
}

/// `interpolation_search_level` 4 is `IFS_MDS3` (enc_mode_config.c:4086),
/// the level the video ladder assigns every preset the port accepts; the
/// MDS3 hook (`leaf_funnel::ifs`) keys on exactly this.
#[test]
fn campaign_ifs_level_is_mds3() {
    use crate::port_enc_mode_config::ctrls::IfsLevel;
    assert_eq!(cfg().ifs_level, IfsLevel::Mds3);
    assert!(!cfg().ifs_at_mds0);
}

/// POSITIVE CONTROL — the whole point of this module, and it fails if
/// either half is deleted.
///
/// With C's measured ME shape (a LIST-1 candidate and nothing for list
/// 0) the two references reach `pme_search` in DIFFERENT states, and
/// that difference is what makes the reference set and PME one
/// mechanism:
///
/// * `BWDREF` has ME data, so `read_refine_me_mvs` refines its MV and
///   `pme_search` takes a BAIL-TO-ME exit — the PME MV is an ECHO of the
///   ME MV, not a search result;
/// * `LAST` has NO ME data, so the full-pel search actually RUNS
///   (`PmeExit::Searched`) and is the ONLY producer of a LAST_FRAME
///   NEWMV candidate. Delete the `pme_search` half and `valid_pme_mv[0]`
///   is false; delete the reference-set half and the loop never visits
///   list 0 at all. Either way this assertion fails.
#[test]
fn pme_is_the_only_producer_of_a_last_frame_mv_when_me_has_only_list_1() {
    let src = source();
    let refp = PaddedRef {
        y: PaddedPlane::from_plane(&reference(), W, H, 64),
        uv: None,
        // 8-bit fixture: no 10-bit twin, which is what every u8 encode
        // puts in the DPB.
        hbd: None,
    };
    let padded_by_ref: [Option<&PaddedRef>; 8] =
        [None, Some(&refp), None, None, None, Some(&refp), None, None];
    let stacks = alloc::vec![crate::inter_mvp::InterMvpStack::default(); 8];
    let nmv = zero_cost();
    let fac = [[0i32; 2]; DRL_MODE_CONTEXTS];
    let tables = crate::intrabc::build_nmv_cost_table(
        &crate::entropy::context::FrameContext::new_default().nmvc,
        crate::entropy::mv_coding::MvSubpelPrecision::Low,
    );
    let me = frame_me_list1_only();
    let out = run_block_searches(
        &cfg(),
        &BlockSearchIn {
            // The measured frame-1 lambdas of `gradient 128x128 q40 p8`,
            // which is the cell this fixture reproduces.
            full_lambda_8bit: 241_378,
            fast_lambda_8bit: 6_633,
            org_x: 0,
            org_y: 0,
            bw: 64,
            bh: 64,
            // BLOCK_64X64
            bsize: 12,
            sq_size: 64,
            mi_rows: (H / 4) as i32,
            mi_cols: (W / 4) as i32,
            src: &src,
            src_stride: W,
            ref_frame_type_arr: &[1, 5],
            padded_by_ref: &padded_by_ref,
            stacks: &stacks,
            ref_mv_count: &[0; 8],
            nmv: &nmv,
            drl_mode_fac_bits: &fac,
            search_tables: &tables,
            me: &me,
            // No square parent: this positive control drives a 64x64
            // square, which is the block that WRITES the state.
            sq_me: None,
            // Not intra-bordered: the PME arm this test exercises.
            updated_enable_pme: true,
        },
    );

    let summary = alloc::format!(
        "l0: sbme={:?} pme={:?} valid={} exit={:?} | l1: sbme={:?} pme={:?} valid={} exit={:?}",
        out.sb_me_mv[0][0],
        out.best_pme_mv[0][0],
        out.valid_pme_mv[0][0],
        out.pme_exit[0][0],
        out.sb_me_mv[1][0],
        out.best_pme_mv[1][0],
        out.valid_pme_mv[1][0],
        out.pme_exit[1][0],
    );

    // BWDREF: the ME MV was refined and PME echoed it.
    assert_eq!(
        out.sb_me_mv[1][0],
        Mv {
            x: -(SHIFT as i16) * 8,
            y: 0
        },
        "list 1's ME MV is the one C's candidate array names"
    );
    assert!(out.valid_pme_mv[1][0]);
    assert!(
        matches!(
            out.pme_exit[1][0],
            Some(PmeExit::EarlyMvpCheck | PmeExit::PreFullPel | PmeExit::PostFullPel)
        ),
        "list 1 has ME data, so PME must take a bail-to-ME exit, not run a \
             search — {summary}"
    );

    // LAST: no ME data at all, so the SEARCH is what produces the MV.
    assert_eq!(
        out.sb_me_mv[0][0],
        Mv::ZERO,
        "list 0 has no ME data, so read_refine_me_mvs must leave sb_me_mv alone"
    );
    assert_eq!(
        out.pme_exit[0][0],
        Some(PmeExit::Searched),
        "list 0's PME must RUN its full-pel search — a bail exit here would \
             mean the driver found ME data C does not have"
    );
    assert!(out.valid_pme_mv[0][0]);
    assert_eq!(
        out.best_pme_mv[0][0].x,
        -(SHIFT as i16) * 8,
        "the PME search must land on the true displacement, not on the \
             zero MVP it started from"
    );
}

/// NEGATIVE CONTROL for the per-block gate — C's
/// `ctx->updated_enable_pme` is zeroed when `is_intra_bordered &&
/// use_neighbouring_mode_ctrls.enabled` (product_coding_loop.c:
/// 9419-9422), and on `vidyo1 256x256 p8` frame 1 the port's missing
/// zeroing let an intra-bordered 8x8 run `pme_search` and inject the
/// NEWMV/NEWNEWMV candidates C suppressed. With the flag cleared the
/// same fixture must produce NO PME output at all while the ME
/// refinement still lands.
#[test]
fn updated_enable_pme_false_suppresses_pme_search_but_not_me() {
    let src = source();
    let refp = PaddedRef {
        y: PaddedPlane::from_plane(&reference(), W, H, 64),
        uv: None,
        hbd: None,
    };
    let padded_by_ref: [Option<&PaddedRef>; 8] =
        [None, Some(&refp), None, None, None, Some(&refp), None, None];
    let stacks = alloc::vec![crate::inter_mvp::InterMvpStack::default(); 8];
    let nmv = zero_cost();
    let fac = [[0i32; 2]; DRL_MODE_CONTEXTS];
    let tables = crate::intrabc::build_nmv_cost_table(
        &crate::entropy::context::FrameContext::new_default().nmvc,
        crate::entropy::mv_coding::MvSubpelPrecision::Low,
    );
    let me = frame_me_list1_only();
    let out = run_block_searches(
        &cfg(),
        &BlockSearchIn {
            full_lambda_8bit: 241_378,
            fast_lambda_8bit: 6_633,
            org_x: 0,
            org_y: 0,
            bw: 64,
            bh: 64,
            bsize: 12,
            sq_size: 64,
            mi_rows: (H / 4) as i32,
            mi_cols: (W / 4) as i32,
            src: &src,
            src_stride: W,
            ref_frame_type_arr: &[1, 5],
            padded_by_ref: &padded_by_ref,
            stacks: &stacks,
            ref_mv_count: &[0; 8],
            nmv: &nmv,
            drl_mode_fac_bits: &fac,
            search_tables: &tables,
            me: &me,
            sq_me: None,
            // Intra-bordered: C's second assignment cleared it.
            updated_enable_pme: false,
        },
    );

    assert!(
        out.pme_exit.iter().flatten().all(Option::is_none)
            && out.valid_pme_mv.iter().flatten().all(|v| !*v)
            && out.best_pme_mv.iter().flatten().all(|&m| m == Mv::ZERO),
        "updated_enable_pme=0 must skip pme_search entirely"
    );
    assert_eq!(
        out.sb_me_mv[1][0],
        Mv {
            x: -(SHIFT as i16) * 8,
            y: 0
        },
        "the ME refinement is not gated on updated_enable_pme"
    );
}
