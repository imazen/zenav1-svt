//! Fork chroma-qindex derivation (SVT_HDR_MODE=1 `rc_crf_cqp.c` block).
//!
//! The svt-av1-hdr fork applies UNCONDITIONAL chroma boosts on every encode
//! (this was the single largest bitstream divergence in the C hybrid work —
//! it flips `separate_uv_delta_q`/`diff_uv_delta` and adds +4 bytes/frame):
//!
//! * general 4:2:0 boost with ramp-down: `-= CLIP3(0, 8, adj/2)`
//! * PQ transfer (SMPTE 2084):           `-= CLIP3(0, 4, adj/6 - 8)`
//! * P3 primaries (SMPTE 431/432):       `-= CLIP3(0, 4, adj/6 - 8)`
//! * BT.2020 primaries:                  `-= CLIP3(0, 8, adj/6 - 8)`
//! * Cb (U) delta gets a further `+12`.
//!
//! where `adj` = the chroma qindex after per-layer offsets (== `new_qindex`
//! in this port: no chroma offsets configured), and the result clamps to
//! the min/max-QP qindex range ([0,255] at default min/max_qp_allowed).
//!
//! Tune-specific branches (TUNE_SSIM pow-curve, TUNE_IQ) are not reachable
//! in this port's envelope (no tune config; C default tune=1/PSNR hits no
//! case) and are intentionally not carried — revisit if tune lands.
//!
//! ACTIVATION STATUS: derivation + SH/FH syntax are capability-complete and
//! unit-tested; the pipeline does NOT yet signal them because the chroma
//! QUANT path still consumes a single qindex for both planes. Signaling
//! deltas the quantizer doesn't apply would desync every decoder, so the
//! switch-on happens together with the per-plane quant threading (task #3
//! remainder; see docs/HDR-ON-4.2.md).

use svtav1_entropy::obu::ColorDescription;

/// CICP constants (EbSvtAv1Formats.h).
const EB_CICP_TC_SMPTE_2084: u8 = 16;
const EB_CICP_CP_BT_2020: u8 = 9;
const EB_CICP_CP_SMPTE_431: u8 = 11;
const EB_CICP_CP_SMPTE_432: u8 = 12;

/// Per-plane chroma delta-q set, FH order: [U dc, U ac, V dc, V ac].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChromaQDeltas {
    pub u_dc: i8,
    pub u_ac: i8,
    pub v_dc: i8,
    pub v_ac: i8,
}

impl ChromaQDeltas {
    /// True when every delta is zero (mainline FH bit pattern applies).
    pub fn is_zero(&self) -> bool {
        *self == Self::default()
    }
}

#[inline]
fn clip3(lo: i32, hi: i32, v: i32) -> i32 {
    v.clamp(lo, hi)
}

/// The fork's chroma qindex derivation for the port's envelope
/// (no per-layer chroma offsets, no tune). Returns the FH delta set.
pub fn fork_chroma_q_deltas(new_qindex: u8, color: &ColorDescription) -> ChromaQDeltas {
    let new_qindex = i32::from(new_qindex);
    let mut chroma_qindex = new_qindex;
    let adj = chroma_qindex;

    // Tune-independent chroma boosts (fork block, rc_crf_cqp.c).
    chroma_qindex -= clip3(0, 8, adj / 2);
    if color.transfer_characteristics == EB_CICP_TC_SMPTE_2084 {
        chroma_qindex -= clip3(0, 4, adj / 6 - 8);
    }
    if color.color_primaries == EB_CICP_CP_SMPTE_431
        || color.color_primaries == EB_CICP_CP_SMPTE_432
    {
        chroma_qindex -= clip3(0, 4, adj / 6 - 8);
    }
    if color.color_primaries == EB_CICP_CP_BT_2020 {
        chroma_qindex -= clip3(0, 8, adj / 6 - 8);
    }
    // clamp_qindex at default min/max_qp_allowed = [0, 255].
    chroma_qindex = chroma_qindex.clamp(0, 255);

    let u = clip3(-64, 63, chroma_qindex - new_qindex + 12) as i8;
    let v = clip3(-64, 63, chroma_qindex - new_qindex) as i8;
    ChromaQDeltas {
        u_dc: u,
        u_ac: u,
        v_dc: v,
        v_ac: v,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cd(cp: u8, tc: u8) -> ColorDescription {
        ColorDescription {
            color_primaries: cp,
            transfer_characteristics: tc,
            matrix_coefficients: 2,
            full_range: false,
        }
    }

    #[test]
    fn sdr_srgb_boost_only_general() {
        // qindex 100: adj/2 = 50 -> clip 8; U = -8 + 12 = +4, V = -8.
        let d = fork_chroma_q_deltas(100, &cd(1, 13));
        assert_eq!((d.u_dc, d.u_ac, d.v_dc, d.v_ac), (4, 4, -8, -8));
    }

    #[test]
    fn low_qindex_ramp_down() {
        // qindex 10: adj/2 = 5 (< 8 cap); U = -5+12 = 7, V = -5.
        let d = fork_chroma_q_deltas(10, &cd(1, 13));
        assert_eq!((d.u_dc, d.v_dc), (7, -5));
        // qindex 0: no boost at all; U = +12, V = 0.
        let d = fork_chroma_q_deltas(0, &cd(1, 13));
        assert_eq!((d.u_dc, d.v_dc), (12, 0));
    }

    #[test]
    fn pq_and_wide_gamut_stack() {
        // qindex 240, PQ + BT.2020: adj/6-8 = 32 -> caps 4 (PQ) + 8 (2020);
        // general 8. total -20; U = -20+12 = -8, V = -20.
        let d = fork_chroma_q_deltas(240, &cd(EB_CICP_CP_BT_2020, EB_CICP_TC_SMPTE_2084));
        assert_eq!((d.u_dc, d.v_dc), (-8, -20));
        // P3 caps at 4.
        let d = fork_chroma_q_deltas(240, &cd(EB_CICP_CP_SMPTE_431, 13));
        assert_eq!((d.u_dc, d.v_dc), (-8 - 4 + 12, -12 - 4 + 4));
    }
}
