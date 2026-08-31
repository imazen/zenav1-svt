//! Issue #5, chunk 1 of the coded-lossless envelope: the FRAME HEADER at
//! `base_q_idx == 0` must be byte-identical to the C reference's.
//!
//! What CodedLossless changes in the header (spec 5.9.11 / 5.9.19 / 5.9.20 /
//! 5.9.21; C `write_uncompressed_header_obu`, entropy_coding.c:3594-3612):
//! no `loop_filter_params()` bits, no `cdef_params()` bits, no `lr_params()`
//! bits (AllLossless, unscaled frame), no `tx_mode_select` bit (TxMode is
//! ONLY_4X4), and `delta_q_present` is not read at `base_q_idx == 0`. The
//! quantization, segmentation, film-grain and tile blocks are unchanged.
//!
//! ORACLE: `tests/data/c_gradient64_p7_qp{0,1}.obu` — the in-tree C encoder
//! (v4.2.0 mainline, `tools/capture_c_trace/capture_c_trace 64 64 <qp> 7
//! g64.yuv out.obu`, still/AVIF CQP config) on the identity harness's
//! `gradient` 64x64 luma (`((r*255)/h) ^ ((c*3)&0x3f)`) with flat-128 chroma.
//! The qp-0 stream was checked to be genuinely LOSSLESS before it was
//! committed as an oracle (`aomdec --rawvideo` output == the .yuv source,
//! 6144/6144 bytes) — docs/SUSPECTED-C-BUGS.md #1 warns that a qp-0 capture
//! with variance boost ON is internally inconsistent; the mainline default is
//! off, and the lossless decode is the proof.
//!
//! What this does NOT cover (the tile half, still refused by
//! `encode_frame_impl`): TX_4X4-only coding with no tx_size / tx_type symbols
//! and WHT residuals. Until that is ported and byte-verified, `EncodePipeline`
//! keeps returning `UnsupportedConfig` for QP 0.

use svtav1_encoder::cdef::pick_cdef_params_key_frame;
use svtav1_encoder::deblock::pick_filter_levels_key_frame;
use svtav1_encoder::entropy::obu::{
    CdefSignal, ColorDescription, LrSignal, ScSignal, write_key_frame_header_full_lr_sb,
    write_sequence_header_ex, write_temporal_delimiter,
};
use svtav1_encoder::speed_config::seq_tools_for_preset;

const C_QP0: &[u8] = include_bytes!("data/c_gradient64_p7_qp0.obu");
const C_QP1: &[u8] = include_bytes!("data/c_gradient64_p7_qp1.obu");

/// Split an AV1 low-overhead OBU stream into `(obu_type, payload)`.
fn split_obus(mut s: &[u8]) -> Vec<(u8, &[u8])> {
    let mut out = Vec::new();
    while !s.is_empty() {
        let h = s[0];
        assert_eq!(h & 0x80, 0, "forbidden bit");
        let obu_type = (h >> 3) & 0xF;
        let has_ext = h & 0x4 != 0;
        let has_size = h & 0x2 != 0;
        assert!(has_size, "low-overhead streams carry obu_size");
        let mut i = 1 + usize::from(has_ext);
        // leb128
        let mut size = 0usize;
        for k in 0..8 {
            let b = s[i];
            i += 1;
            size |= usize::from(b & 0x7f) << (7 * k);
            if b & 0x80 == 0 {
                break;
            }
        }
        out.push((obu_type, &s[i..i + size]));
        s = &s[i + size..];
    }
    out
}

const OBU_SEQUENCE_HEADER: u8 = 1;
const OBU_FRAME: u8 = 6;

/// The port's frame header for the C cell at `qp` (CLI qp -> qindex via the
/// same 4x mapping the pipeline uses: qp 0 -> 0, qp 1 -> 4), preset 7,
/// 64x64 4:2:0 8-bit, single tile, SB64. At preset 7 the port signals the
/// qp-picked deblock levels and the `svt_pick_cdef_from_qp` strengths (no
/// searches) and all-NONE restoration with the SH `enable_restoration` bit
/// on — exactly what the pipeline passes for this cell.
fn port_frame_header(qindex: u8) -> Vec<u8> {
    let lf = pick_filter_levels_key_frame(qindex, 8);
    let cdef = pick_cdef_params_key_frame(qindex, 8, false);
    let cdef_signal: CdefSignal = svtav1_encoder::cdef::CdefPick::single(cdef).signal();
    let tools = seq_tools_for_preset(7, true, 64 * 64);
    write_key_frame_header_full_lr_sb(
        64,
        64,
        qindex,
        true,  // reduced still-picture header
        false, // 4:2:0, not monochrome
        lf.levels,
        0, // sharpness (mainline)
        &cdef_signal,
        &LrSignal::none(tools.enable_restoration),
        ScSignal::default(),
        None, // mainline chroma-q deltas
        None, // no per-SB delta-q
        None, // no quant matrices
        None, // no film grain
        0,
        0,
        0,
        64,
    )
}

fn port_sequence_header() -> Vec<u8> {
    write_sequence_header_ex(
        64,
        64,
        true,
        8,
        &ColorDescription::default(),
        false,
        30.0,
        seq_tools_for_preset(7, true, 64 * 64),
    )
}

/// Harness control: on the qp-1 capture (a LOSSY header every existing gate
/// already byte-matches through the pipeline) the same hand-built parameter
/// set must reproduce C's temporal delimiter, sequence header OBU, and the
/// frame-header prefix of the frame OBU. If this fails the qp-0 test below
/// would be testing the wrong parameters, not the lossless syntax.
#[test]
fn control_qp1_header_matches_c_capture() {
    let obus = split_obus(C_QP1);
    assert_eq!(obus[0].0, 2, "temporal delimiter first");
    assert!(obus[0].1.is_empty());
    assert_eq!(&write_temporal_delimiter()[..], &C_QP1[..2]);
    let (t, sh_payload) = obus[1];
    assert_eq!(t, OBU_SEQUENCE_HEADER);
    let sh = port_sequence_header();
    // The port returns the whole OBU (header + size + payload).
    assert_eq!(&sh[2..], sh_payload, "sequence header payload");
    let (t, frame) = obus[2];
    assert_eq!(t, OBU_FRAME);
    let fh = port_frame_header(4);
    assert!(
        frame.starts_with(&fh),
        "qp1 frame header must be a prefix of C's frame OBU\nport {:02x?}\nC    {:02x?}",
        fh,
        &frame[..fh.len().min(frame.len())]
    );
}

/// The lossless header: at `base_q_idx == 0` the port must drop the
/// loop-filter, cdef, restoration and tx_mode syntax exactly like C.
#[test]
fn qp0_coded_lossless_frame_header_matches_c_capture() {
    let obus = split_obus(C_QP0);
    assert_eq!(&write_temporal_delimiter()[..], &C_QP0[..2]);
    let (t, sh_payload) = obus[1];
    assert_eq!(t, OBU_SEQUENCE_HEADER);
    assert_eq!(
        &port_sequence_header()[2..],
        sh_payload,
        "sequence header payload"
    );
    let (t, frame) = obus[2];
    assert_eq!(t, OBU_FRAME);
    let fh0 = port_frame_header(0);
    assert!(
        frame.starts_with(&fh0),
        "qp0 frame header must be a prefix of C's frame OBU\nport {:02x?}\nC    {:02x?}",
        fh0,
        &frame[..fh0.len().min(frame.len())]
    );
    // Anti-vacuity: the lossless header is STRICTLY SHORTER than the lossy
    // one built from the same parameter set — the dropped blocks are real
    // bits (LF 16, CDEF 16 at cdef_bits 0, LR 6, tx_mode 1 = 39 bits here),
    // so a writer that ignored CodedLossless could not pass the prefix check
    // above and this check at once.
    let fh1 = port_frame_header(4);
    assert!(
        fh0.len() < fh1.len(),
        "lossless header ({} B) must be shorter than the lossy one ({} B)",
        fh0.len(),
        fh1.len()
    );
}

/// The identity harness's `gradient` content (`svtav1/examples/identity_run.rs`:
/// `y[r][c] = ((r*255/h) ^ ((c*3) & 0x3f))`, flat 128 chroma) — the exact
/// frame `capture_c_trace 64 64 <qp> 7` was fed for both committed captures.
fn gradient_64() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (w, h) = (64usize, 64usize);
    let mut y = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            y[r * w + c] = (((r * 255) / h) as u8) ^ (((c * 3) & 0x3f) as u8);
        }
    }
    (y, vec![128u8; w * h / 4], vec![128u8; w * h / 4])
}

fn encode_gradient_64(qp: u8) -> (Vec<u8>, svtav1_encoder::pipeline::EncodePipeline) {
    use svtav1_encoder::pipeline::EncodePipeline;
    use svtav1_encoder::rate_control::{RcConfig, RcMode};
    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp,
        ..RcConfig::default()
    };
    let mut p = EncodePipeline::new(64, 64, 7, rc, 0, 1)
        .with_chroma_420(true)
        .with_recon_output(true);
    let (y, u, v) = gradient_64();
    let obu = p
        .try_encode_frame_420(&y, &u, &v, 64)
        .expect("gradient 64x64 preset 7 encodes");
    (obu, p)
}

/// Control for the stream-level test below: the qp-1 (LOSSY) stream through
/// the real pipeline is byte-identical to C's capture, so the encoder
/// configuration the qp-0 test runs is the one the oracle was captured with.
#[test]
fn control_qp1_stream_matches_c_capture() {
    let (obu, _) = encode_gradient_64(1);
    assert_eq!(obu.len(), C_QP1.len(), "qp1 stream length");
    assert!(obu == C_QP1, "qp1 stream must be byte-identical to C");
}

/// Issue #5 chunk 2 — THE gate: the qp-0 coded-lossless stream (TX_4X4 WHT
/// txbs, no tx_size / tx_type symbols, no in-loop filters, DC/PAETH-only
/// injection) is byte-identical to the C encoder's, and the encoder's own
/// reconstruction equals the source (the capture was verified to decode
/// losslessly under aomdec before adoption, so equality to it is a lossless
/// decode by transitivity). MUTATION-VERIFIED: with the WHT arm replaced by
/// the DCT, or the tx_size symbol coded, the bytes diverge.
#[test]
fn qp0_coded_lossless_stream_matches_c_capture() {
    let (obu, p) = encode_gradient_64(0);
    let (y, u, v) = gradient_64();
    let (ry, ru, rv) = p.last_recon.as_ref().expect("recon_output");
    assert_eq!(&ry[..], &y[..], "luma recon == source");
    assert_eq!(&ru[..], &u[..], "Cb recon == source");
    assert_eq!(&rv[..], &v[..], "Cr recon == source");
    if obu != C_QP0 {
        let first = obu
            .iter()
            .zip(C_QP0.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(obu.len().min(C_QP0.len()));
        panic!(
            "qp0 stream differs from C: port {} B, C {} B, first divergent byte at {first}\nport {:02x?}\nC    {:02x?}",
            obu.len(),
            C_QP0.len(),
            &obu[first.saturating_sub(8)..(first + 8).min(obu.len())],
            &C_QP0[first.saturating_sub(8)..(first + 8).min(C_QP0.len())]
        );
    }
}
