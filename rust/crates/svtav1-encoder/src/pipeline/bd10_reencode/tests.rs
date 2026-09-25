use super::*;
use crate::partition::{BlockDecision, InterDecision, PartitionTree};
use crate::picture::{PaddedPlane, PaddedPlaneHbd, PaddedRef, PaddedRefHbd};
use svtav1_types::motion::{Mv, WarpedMotionParams};
use svtav1_types::prediction::PredictionMode;

/// A 32x32 leaf on a 32x32 frame, driven through the depth-0 arm. The
/// reference is flat so the prediction is flat; the source carries a hard
/// gradient so that any residual the re-quantize computes is nonzero.
/// `skip_mode` flips the committed-syntax contract between the arms
/// exercised below; `committed` decides whether the leaf's MD decision
/// carries coefficients (C's `blk_skip_decision` / `md_skip_blk`).
fn run_leaf(skip_mode: bool, committed: bool) -> (BlockDecision, alloc::vec::Vec<u16>) {
    const W: usize = 32;
    const H: usize = 32;
    let bd = 10u8;
    let ref_y = alloc::vec![512u16; W * H];
    let ref_c = alloc::vec![512u16; W * H / 4];
    let pref = PaddedRef {
        y: PaddedPlane::from_plane(&alloc::vec![128u8; W * H], W, H, 32),
        uv: Some((
            PaddedPlane::from_plane(&alloc::vec![128u8; W * H / 4], W / 2, H / 2, 16),
            PaddedPlane::from_plane(&alloc::vec![128u8; W * H / 4], W / 2, H / 2, 16),
        )),
        hbd: Some(PaddedRefHbd {
            y: PaddedPlaneHbd::from_plane(&ref_y, W, H, 32),
            uv: Some((
                PaddedPlaneHbd::from_plane(&ref_c, W / 2, H / 2, 16),
                PaddedPlaneHbd::from_plane(&ref_c, W / 2, H / 2, 16),
            )),
        }),
    };
    let refs: [Option<&PaddedRef>; 8] = [Some(&pref); 8];
    // Strong slope: src - pred reaches ~+500 at the far edge, which no
    // quantizer can drop to zero.
    let src10: alloc::vec::Vec<u16> = (0..W * H)
        .map(|i| (512 + (i % W) * 24).min(1023) as u16)
        .collect();
    let inter = InterDecision {
        mode: PredictionMode::NearestNearestMv,
        ref_frame: [1, 7],
        mv: [Mv { x: 0, y: 0 }, Mv { x: 0, y: 0 }],
        drl_index: 0,
        interp_filters: 0,
        motion_mode: crate::port_entropy_inter::modes::MotionMode::SimpleTranslation,
        num_proj_ref: 0,
        overlappable_neighbors: 0,
        skip_mode,
        comp_group_idx: 0,
        compound_idx: 1,
        interinter_comp_type: 0,
        interinter_mask_type: 0,
        interinter_wedge_index: 0,
        interinter_wedge_sign: false,
        is_interintra_used: false,
        interintra_mode: 0,
        use_wedge_interintra: false,
        interintra_wedge_index: 0,
        wm_params: WarpedMotionParams::default(),
        wm_params_l1: WarpedMotionParams::default(),
    };
    let mut tree = PartitionTree::Leaf(BlockDecision {
        is_inter: true,
        inter: Some(alloc::boxed::Box::new(inter)),
        width: W as u16,
        height: H as u16,
        qcoeffs: {
            let mut q = alloc::vec![0i32; W * H];
            if committed {
                q[0] = 1;
            }
            q
        },
        eob: u16::from(committed),
        ..Default::default()
    });
    let fc = crate::entropy::context::FrameContext::new_default();
    let cfc = crate::entropy::coeff_c::CoeffFc::default_for_qindex(40);
    let rates = crate::leaf_funnel::build_md_rates(&fc, &cfc);
    let qt = crate::quant::build_quant_table_bd_sharp(40, bd, 0);
    let mut recon10 = alloc::vec![128u16 << 2; W * H];
    let mut cn = Bd10CoeffNeighbors::new(W, H).unwrap();
    let mut mn = Bd10ModeNeighbors::new(W, H).unwrap();
    // The leaf is not OBMC, so the grid's contents are never read; it
    // only has to be shaped.
    let mi_grid = alloc::vec![crate::intrabc_mvp::MvpMiEntry::default(); (W / 4) * (H / 4)];
    bd10_reencode_node(
        false,
        16,
        &mut tree,
        0,
        0,
        &mut recon10,
        W,
        &src10,
        W,
        &qt,
        0,
        1 << 10,
        false,
        &rates,
        true,
        &mut cn,
        false,
        W,
        H,
        bd,
        0,
        crate::intra_edge::TileMi::whole_frame(W, H),
        svtav1_types::partition::PartitionType::None,
        Some(&refs),
        &mi_grid,
        W / 4,
        &mut mn,
        &mut alloc::collections::BTreeSet::new(),
    );
    let PartitionTree::Leaf(d) = tree else {
        panic!("a leaf in, a leaf out")
    };
    (d, recon10)
}

/// The witness for the measured tile desync: a `skip_mode` leaf's syntax
/// is committed to a zero residual, so the 10-bit re-encode must NOT
/// resurrect levels the decoder will never read. Before the forced-zero
/// arm this leaf re-quantized the slope into nonzero `eob` while the
/// writer still suppressed the coefficient sections — aomdec reported
/// "Failed to decode tile data" (johnny 256x256 q40 p6 f3, 2026-09-16).
#[test]
fn skip_mode_leaf_keeps_its_committed_zero_residual() {
    // The control arm first: WITHOUT skip_mode the same leaf must
    // re-quantize the slope into real levels, or the witness below could
    // never observe a regression.
    let (d, _) = run_leaf(false, true);
    assert!(
        d.eob > 0,
        "control leaf must produce a residual — a witness that cannot \
             fail is not a witness"
    );
    // `skip_mode` alone forces the zero — even with committed coeffs
    // (coding_loop.c: `md_skip_blk` covers both cases).
    let (d, recon10) = run_leaf(true, true);
    assert_eq!(d.eob, 0, "skip_mode leaf must stay residual-free");
    assert!(
        d.qcoeffs.iter().all(|&c| c == 0),
        "skip_mode leaf must commit all-zero levels"
    );
    // And the reconstruction is the prediction itself — flat 512 from the
    // flat reference — exactly what a decoder reconstructs for the block.
    assert!(
        recon10.iter().all(|&s| s == 512),
        "skip_mode leaf reconstructs as its prediction"
    );
}

/// The second half of C's `md_skip_blk` contract (coding_loop.c:387):
/// an inter leaf whose MD decision committed NO coefficients is
/// force-zeroed in the encode pass — the post-pass must not re-quantize
/// the residual back into existence. Measured: vidyo1 256x256 p6 frame 1
/// diverged only by such resurrected TUs until this arm existed.
#[test]
fn committed_zero_inter_leaf_stays_zero() {
    let (d, recon10) = run_leaf(false, false);
    assert_eq!(d.eob, 0, "committed-all-zero leaf must stay residual-free");
    assert!(
        d.qcoeffs.iter().all(|&c| c == 0),
        "committed-all-zero leaf must commit all-zero levels"
    );
    assert!(
        recon10.iter().all(|&s| s == 512),
        "committed-all-zero leaf reconstructs as its prediction"
    );
}

/// The sub-8 chroma stitch (`predict_inter_chroma_sub8_hbd`) reads the
/// SIBLING's motion out of the mi grid the post-pass rebuilds from the
/// committed tree — `inter_chroma_4xn_pred`'s covered-cell walk. Two 16x4
/// inter leaves stacked under a Horz split are the minimal shape: the
/// second (odd-mi) leaf's chroma covers the pair and must see the first
/// leaf's reference/MV/filter, not a default intra cell.
#[test]
fn stamp_inter_mi_grid_carries_covered_sibling_motion() {
    let leaf = |mv: Mv, rf: i8, filt: u32| {
        PartitionTree::Leaf(BlockDecision {
            is_inter: true,
            inter: Some(alloc::boxed::Box::new(InterDecision {
                mode: PredictionMode::NearestMv,
                ref_frame: [rf, -1],
                mv: [mv, Mv::default()],
                drl_index: 0,
                interp_filters: filt,
                motion_mode: crate::port_entropy_inter::modes::MotionMode::SimpleTranslation,
                num_proj_ref: 0,
                overlappable_neighbors: 0,
                skip_mode: false,
                comp_group_idx: 0,
                compound_idx: 1,
                interinter_comp_type: 0,
                interinter_mask_type: 0,
                interinter_wedge_index: 0,
                interinter_wedge_sign: false,
                is_interintra_used: false,
                interintra_mode: 0,
                use_wedge_interintra: false,
                interintra_wedge_index: 0,
                wm_params: WarpedMotionParams::default(),
                wm_params_l1: WarpedMotionParams::default(),
            })),
            width: 16,
            height: 4,
            ..Default::default()
        })
    };
    // A 16x8 node split Horz into two 16x4 leaves at luma y=0 and y=4 —
    // mi rows 0 and 1, i.e. one chroma pair at chroma (0,0) 8x4.
    let tree = PartitionTree::Split {
        partition_type: crate::partition::PartitionType::Horz,
        width: 16,
        height: 8,
        children: alloc::vec![
            leaf(Mv { x: 8, y: 16 }, 1, 0x10001),
            leaf(Mv { x: -8, y: 0 }, 2, 0x20002),
        ],
    };
    let stride = 4usize; // 16px / 4 = 4 mi cols, 2 mi rows
    let mut grid = alloc::vec![crate::intrabc_mvp::MvpMiEntry::default(); stride * 2];
    stamp_inter_mi_grid(
        &tree,
        0,
        0,
        svtav1_types::partition::PartitionType::Horz as u8,
        &mut grid,
        stride,
        16,
        8,
    );
    // The covered cell the stitcher reads for the second leaf is
    // (mi_row 1 + dr=-1, mi_col 0) -> row 0: the FIRST leaf's motion.
    let e = &grid[0];
    assert!(
        e.use_intrabc || e.ref_frame[0] > 0,
        "sibling must read inter"
    );
    assert_eq!(e.ref_frame[0], 1);
    assert_eq!((e.mv[0].x, e.mv[0].y), (8, 16));
    assert_eq!(e.interp_filters, 0x10001);
    // Row 1 carries the second leaf's own params.
    let e = &grid[stride];
    assert_eq!(e.ref_frame[0], 2);
    assert_eq!((e.mv[0].x, e.mv[0].y), (-8, 0));
    assert_eq!(e.interp_filters, 0x20002);
}

/// The post-pass OBMC arm (`apply_obmc_hbd_postpass`): an `ObmcCausal`
/// leaf's neighbour spans come out of the committed-tree mi grid, the
/// neighbours' 10-bit predictions are rebuilt from the DPB `hbd` twins,
/// and the blend rewrites the block's edge band in place — exactly the
/// funnel's `predict_obmc_in_place_hbd` driven by reconstructed state.
/// The witness is a `skip_mode` leaf (committed zero residual ⇒ recon IS
/// the prediction): with an overlappable inter neighbour above carrying
/// different motion on a non-flat reference, the blended top band must
/// differ from the pure base translation while the interior is untouched.
#[test]
fn obmc_leaf_blends_edges_from_the_committed_grid() {
    const W: usize = 16;
    const H: usize = 16;
    let bd = 10u8;
    // A ramp so the neighbour's displaced prediction differs from the
    // block's own — the blend is only observable when the two differ.
    let ref_y: alloc::vec::Vec<u16> = (0..W * H)
        .map(|i| (((i % W) * 29 + (i / W) * 53) & 1023) as u16)
        .collect();
    let ref_c = alloc::vec![512u16; W * H / 4];
    let pref = PaddedRef {
        y: PaddedPlane::from_plane(&alloc::vec![128u8; W * H], W, H, 32),
        uv: Some((
            PaddedPlane::from_plane(&alloc::vec![128u8; W * H / 4], W / 2, H / 2, 16),
            PaddedPlane::from_plane(&alloc::vec![128u8; W * H / 4], W / 2, H / 2, 16),
        )),
        hbd: Some(PaddedRefHbd {
            y: PaddedPlaneHbd::from_plane(&ref_y, W, H, 32),
            uv: Some((
                PaddedPlaneHbd::from_plane(&ref_c, W / 2, H / 2, 16),
                PaddedPlaneHbd::from_plane(&ref_c, W / 2, H / 2, 16),
            )),
        }),
    };
    let refs: [Option<&PaddedRef>; 8] = [Some(&pref); 8];
    // The committed tree: a 16x8 inter leaf on top carrying DIFFERENT
    // motion (mv.y = 16/8 = 2 px), the 16x8 OBMC leaf below it.
    let leaf = |motion_mode, mv: Mv| {
        PartitionTree::Leaf(BlockDecision {
            is_inter: true,
            inter: Some(alloc::boxed::Box::new(InterDecision {
                mode: PredictionMode::NearestMv,
                ref_frame: [1, -1],
                mv: [mv, Mv::default()],
                drl_index: 0,
                interp_filters: 0,
                motion_mode,
                num_proj_ref: 0,
                overlappable_neighbors: 0,
                skip_mode: true,
                comp_group_idx: 0,
                compound_idx: 1,
                interinter_comp_type: 0,
                interinter_mask_type: 0,
                interinter_wedge_index: 0,
                interinter_wedge_sign: false,
                is_interintra_used: false,
                interintra_mode: 0,
                use_wedge_interintra: false,
                interintra_wedge_index: 0,
                wm_params: WarpedMotionParams::default(),
                wm_params_l1: WarpedMotionParams::default(),
            })),
            width: 16,
            height: 8,
            ..Default::default()
        })
    };
    let tree = PartitionTree::Split {
        partition_type: crate::partition::PartitionType::Horz,
        width: W as u16,
        height: H as u16,
        children: alloc::vec![
            leaf(
                crate::port_entropy_inter::modes::MotionMode::SimpleTranslation,
                Mv { x: 0, y: 16 },
            ),
            leaf(
                crate::port_entropy_inter::modes::MotionMode::ObmcCausal,
                Mv { x: 0, y: 0 },
            ),
        ],
    };
    let mi_stride = W / 4;
    let mut mi_grid = alloc::vec![crate::intrabc_mvp::MvpMiEntry::default(); mi_stride * (H / 4)];
    stamp_inter_mi_grid(
        &tree,
        0,
        0,
        svtav1_types::partition::PartitionType::Horz as u8,
        &mut mi_grid,
        mi_stride,
        W,
        H,
    );
    // The skip_mode contract makes the walk's recon equal to the OBMC
    // prediction: the OBMC leaf's base translation first, then the blend.
    let ic = match &tree {
        PartitionTree::Split { children, .. } => match &children[1] {
            PartitionTree::Leaf(d) => d.inter.as_deref().unwrap(),
            _ => panic!("a leaf in"),
        },
        _ => panic!("a split in"),
    };
    let mut base = alloc::vec![0u16; 16 * 8];
    predict_inter_leaf_hbd_any(
        &refs,
        ic,
        0,
        8,
        16,
        8,
        64,
        W,
        H,
        bd,
        false,
        &mut base,
        16,
        &mut [],
        &mut [],
        0,
    );
    let mut blended = base.clone();
    apply_obmc_hbd_postpass(
        &refs,
        &mi_grid,
        mi_stride,
        ic,
        0,
        8,
        16,
        8,
        64,
        W,
        H,
        bd,
        &mut blended,
        16,
        &mut [],
        &mut [],
        0,
    );
    // The above-blend band is `min(bh,64)/2 = 4` rows deep.
    assert_ne!(
        blended[..4 * 16],
        base[..4 * 16],
        "the top 4 rows must carry the neighbour's blended motion"
    );
    assert_eq!(
        blended[4 * 16..],
        base[4 * 16..],
        "rows below the overlap band keep the block's own prediction"
    );
    // And the chroma-only arm (the chroma walk's call shape — no luma
    // buffer) must not panic and must not touch luma.
    let mut u = alloc::vec![512u16; 8 * 4];
    let mut v = alloc::vec![512u16; 8 * 4];
    apply_obmc_hbd_postpass(
        &refs,
        &mi_grid,
        mi_stride,
        ic,
        0,
        8,
        16,
        8,
        64,
        W,
        H,
        bd,
        &mut [],
        0,
        &mut u,
        &mut v,
        8,
    );
}
