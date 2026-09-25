use super::*;
use crate::entropy::writer::AomWriter;
use crate::port_entropy_inter::Neighbors;
use crate::port_entropy_inter::block::{InterFrameSyntax, InterModeInfo, write_inter_mode_info};
use crate::port_entropy_inter::modes::{MotionMode, TransformationType};
use crate::port_entropy_inter::refframe::ReferenceMode;
use crate::rate_control::RcMode;
use alloc::vec;
use svtav1_types::block::BlockSize;
use svtav1_types::motion::Mv;
use svtav1_types::prediction::PredictionMode;

/// C's `gradient` plane, as `tools/identity_run` builds it.
fn gradient(w: usize, h: usize) -> Vec<u8> {
    (0..h)
        .flat_map(|r| (0..w).map(move |c| (((r * 255) / h) as u8) ^ (((c * 3) & 0x3f) as u8)))
        .collect()
}

/// C's frame-1 TILE for the reference cell — the three bytes after the
/// 2-byte temporal delimiter, the 2-byte OBU header and the 15-byte frame
/// header of a 22-byte frame. Captured from
/// `tools/identity_diff_inter.sh 64 64 40 6 2 gradient`'s `c.obu.pts1`:
///
/// ```text
/// 12 00 32 12 | 30 02 00 80 00 db 3b 40 00 00 04 04 e0 1c 00 | 94 9a b0
/// ```
///
/// The 15 header bytes are already byte-identical to the port's
/// (`tools/inter_fh_gate.sh`, docs/INTER-ENCODE-PLAN.md §1r).
const C_INTER_TILE: [u8; 3] = [0x94, 0x9a, 0xb0];

#[test]
fn the_inter_tile_matches_c_from_cs_measured_decision() {
    let (w, h) = (64usize, 64usize);
    let y = gradient(w, h);
    let uv = vec![128u8; (w / 2) * (h / 2)];

    // The reference cell's configuration, field for field with
    // `tools/identity_run`'s video arm: preset 6, CQP 40, flat GOP
    // (hierarchical_levels 0), intra_period 64, 4:2:0, 8-bit.
    let mut pipeline = EncodePipeline::new(
        w as u32,
        h as u32,
        6,
        RcConfig {
            mode: RcMode::Cqp,
            qp: 40,
            ..RcConfig::default()
        },
        0,
        64,
    )
    .with_bit_depth(8)
    .with_chroma_420(true);

    let f0 = pipeline
        .try_encode_frame_420(&y, &uv, &uv, w)
        .expect("the video-mode key frame encodes");
    // ANTI-VACUITY, and the reason it is an assertion rather than a
    // comment: everything below is read out of the frame-0 encode's
    // state, so a configuration that silently produced a DIFFERENT
    // frame 0 would hand the tile writer plausible-looking CDFs from the
    // wrong encode. 961 B is C's byte count for this cell
    // (docs/INTER-ENCODE-PLAN.md §1q) and the port matches it exactly.
    assert_eq!(
        f0.len(),
        961,
        "this is not the reference cell — frame 0 must be C's 961 bytes"
    );

    // The CDF continuation under test: frame 1's header names
    // primary_ref_frame = 0, and ref_frame_idx[0] = 0, so the tile starts
    // from DPB slot 0's saved end-of-frame state.
    let saved = pipeline
        .dpb
        .get(0)
        .and_then(|r| r.frame_cdfs.clone())
        .expect("the key frame stored its end-of-frame CDFs on the DPB slot it refreshed");
    // The prologue and the mode-info group are assembled by `tile_from`
    // below, so the negative control at the end of this test runs the
    // IDENTICAL path with only the CDF state swapped.
    // --- the inter mode-info group ------------------------------------
    let nb = Neighbors {
        above: None,
        left: None,
        up_available: false,
        left_available: false,
    };
    let gm = [TransformationType::Identity; 8];
    let ref_order_hint = [0i32; 7];
    let frame = InterFrameSyntax {
        // frm_hdr reference_select = 1 (tools/fh_fields.py --index 1).
        reference_mode: ReferenceMode::Select,
        // is_filter_switchable = 1 -> SWITCHABLE, which is
        // SWITCHABLE_FILTERS + 1 = 4, NOT 3 (definitions.h:844-846).
        interpolation_filter: crate::port_enc_mode_config::md_config::SWITCHABLE,
        // The video arm never enables dual filter at any preset
        // (`speed_config.rs`, asserted there for M0..M13).
        enable_dual_filter: false,
        enable_interintra_compound: false,
        enable_masked_compound: false,
        enable_jnt_comp: false,
        enable_order_hint: true,
        order_hint_bits: crate::entropy::obu::ORDER_HINT_BITS,
        is_motion_mode_switchable: true,
        allow_warped_motion: true,
        allow_high_precision_mv: false,
        force_integer_mv: false,
        gm_wmtype: &gm,
        cur_order_hint: 1,
        ref_order_hint: &ref_order_hint,
    };
    // C's measured decision — see this module's doc comment.
    let blk = InterModeInfo {
        bsize: BlockSize::Block64x64,
        mode: PredictionMode::NewMv,
        ref_frame: [1, -1], // LAST_FRAME, NONE
        mv: [Mv { x: -24, y: 0 }, Mv { x: 0, y: 0 }],
        pred_mv: [Mv { x: 0, y: 0 }, Mv { x: 0, y: 0 }],
        inter_mode_ctx: 8,
        drl: crate::port_entropy_inter::modes::DrlBlock {
            drl_ctx: [-1, -1],
            drl_ctx_near: [0, 0],
            drl_index: 0,
        },
        interintra: None,
        motion_mode: MotionMode::SimpleTranslation,
        num_proj_ref: 0,
        overlappable_neighbors: 0,
        compound: None,
        interp_filters: 0, // EIGHTTAP_REGULAR in both directions
        skip_mode: false,
    };
    // `FrameContext` carries `inter` and `nmvc` inline, and the writer
    // takes them as separate `&mut`s (C reaches them through one `fc`
    // pointer). Split the copies out, then write them BACK — a caller
    // that dropped them would silently code the next block against
    // unadapted inter CDFs.
    let tile = tile_from(&saved, &nb, &frame, &blk);
    assert_eq!(
        tile.as_slice(),
        &C_INTER_TILE[..],
        "the inter tile does not match C's; port {tile:02x?} vs C {C_INTER_TILE:02x?}"
    );

    // NEGATIVE CONTROL, and it is the reason this test is evidence about
    // CDF CONTINUATION rather than only about the writers: the SAME
    // decision coded from DEFAULT CDFs produces different bytes. If it
    // did not, the gate would pass with the restore deleted and would be
    // saying nothing about the feature it was built for.
    let from_defaults = tile_from(
        &crate::port_frame_cdf::FrameCdfs {
            fc: crate::entropy::context::FrameContext::new_default(),
            coeff: crate::entropy::coeff_c::CoeffFc::default_for_qindex(160),
        },
        &nb,
        &frame,
        &blk,
    );
    assert_ne!(
        from_defaults.as_slice(),
        &C_INTER_TILE[..],
        "default CDFs reproduced C's tile — this gate cannot see the restore"
    );
}

/// The tile assembly of the test above, factored so the negative control
/// runs the IDENTICAL path with only the CDF state swapped. A second
/// hand-written copy could differ somewhere else and hide the point.
fn tile_from(
    cdfs: &crate::port_frame_cdf::FrameCdfs,
    nb: &Neighbors,
    frame: &InterFrameSyntax<'_>,
    blk: &InterModeInfo,
) -> Vec<u8> {
    let mut fc = cdfs.fc.clone();
    let mut w_ = AomWriter::new(256);
    let ectx = EntropyCtx::new(
        16,
        16,
        false,
        true,
        false,
        8,
        svtav1_types::chroma::ChromaFormat::Yuv420,
    );
    let (part_ctx, nsymbs) = ectx.partition_ctx(0, 0, 64);
    crate::entropy::context::write_partition_edge(
        &mut w_, &mut fc, part_ctx, 0, nsymbs, false, true, true,
    );
    crate::entropy::context::write_skip(&mut w_, &mut fc, 0, true);
    crate::entropy::context::write_intra_inter(&mut w_, &mut fc, 0, true);
    write_inter_mode_info(
        &mut w_,
        &mut crate::port_entropy_inter::refframe::RefFrameCdfs {
            comp_inter_cdf: &mut fc.comp_inter_cdf,
            comp_ref_cdf: &mut fc.comp_ref_cdf,
            single_ref_cdf: &mut fc.single_ref_cdf,
        },
        &mut fc.inter,
        &mut fc.nmvc,
        nb,
        frame,
        blk,
    );
    w_.done().to_vec()
}
