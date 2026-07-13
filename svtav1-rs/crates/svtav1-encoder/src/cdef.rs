//! CDEF: frame-level strength/damping selection and (further down, once the
//! application port lands) the decoder-exact frame filter pass.
//!
//! Strength selection: C-exact port of the closed-form QP predictor
//! `svt_pick_cdef_from_qp` (Source/Lib/Codec/enc_cdef.c:849) specialized to
//! its intra branch (`frame_type == KEY_FRAME`), i.e. the C encoder's
//! `use_qp_strength` fast path (enc_cdef.c:1143), which signals
//! `cdef_bits = 0` with a single strength pair — NOT the C default preset
//! policy (the RDO `svt_av1_cdef_search`), which lands with decision parity.
//! Damping: `CDEF_DAMPING_FROM_QP` = `3 + (base_q_idx >> 6)`
//! (enc_cdef.c:923, also inlined at enc_cdef.c:1154/1256/1443).

use svtav1_dsp::quant_tables::AC_QLOOKUP_8;

/// Frame-level CDEF parameters as signaled in `cdef_params()` (spec 5.9.19)
/// with `cdef_bits = 0`: one packed 6-bit strength per plane type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CdefFrameParams {
    /// `cdef_damping`: 3..=6 (`cdef_damping_minus_3` is a 2-bit field).
    pub damping: u8,
    /// Packed luma strength: `pri * 4 + sec_signaled` (0..=63).
    pub y_strength: u8,
    /// Packed chroma strength (ignored / not signaled for monochrome).
    pub uv_strength: u8,
}

impl Default for CdefFrameParams {
    /// All-zero strengths with the minimum legal damping — the decoder's
    /// `do_cdef` gate (libaom decodeframe.c:5417) is then false and the
    /// CDEF pass is skipped entirely on both sides.
    fn default() -> Self {
        Self {
            damping: 3,
            y_strength: 0,
            uv_strength: 0,
        }
    }
}

impl CdefFrameParams {
    /// True when a conforming decoder will run the CDEF frame pass: libaom
    /// `do_cdef = cdef_bits || cdef_strengths[0] || cdef_uv_strengths[0]`
    /// (decodeframe.c:5417; we always signal cdef_bits = 0). Monochrome
    /// streams never signal uv_strength, so the decoder sees 0 there.
    pub fn any(&self, monochrome: bool) -> bool {
        self.y_strength != 0 || (!monochrome && self.uv_strength != 0)
    }

    /// The `[damping, y_strength, uv_strength]` triple the frame-header
    /// writer takes.
    pub fn signal(&self) -> [u8; 3] {
        [self.damping, self.y_strength, self.uv_strength]
    }
}

/// C-exact port of `svt_pick_cdef_from_qp` (enc_cdef.c:849), intra branch
/// (`is_screen_content = 0`, `frame_type == KEY_FRAME`), plus the
/// `CDEF_DAMPING_FROM_QP` damping derivation (enc_cdef.c:923).
///
/// `q = svt_aom_ac_quant_qtx(base_q_idx, 0, 8) >> 0` is the 8-bit AC step;
/// the strength fits are evaluated in f32 exactly as C does (float
/// constants, left-associated sum, `roundf` = round-half-away-from-zero,
/// which is `f32::round`), then clamped to the 4-/2-bit field ranges and
/// packed `f1 * CDEF_SEC_STRENGTHS + f2`.
///
/// Firing profile on the AC table (C-verified in tests +
/// tests/c_parity_cdef_pick.rs): zero at very low qindex (<= ~50, CDEF
/// hurts near-lossless), luma pri kicks in around the qindex-60 knee
/// (y = 4 at qindex 63/80), growing to y = 9/17/43 at qindex 128/172/220
/// and saturating at 63 (pri 15 / sec field 3) at qindex 255.
pub fn pick_cdef_params_key_frame(qindex: u8) -> CdefFrameParams {
    let q = AC_QLOOKUP_8[qindex as usize] as i32 as f32;

    // enc_cdef.c:880-888 (Intra branch), verbatim constants.
    let y_f1 = (q * q * 0.000_003_373_197_4_f32 + q * 0.008_070_594_f32 + 0.018_763_4_f32).round()
        as i32;
    let y_f2 = (q * q * 0.000_002_916_734_3_f32 + q * 0.002_779_862_4_f32 + 0.007_940_5_f32)
        .round() as i32;
    let uv_f1 = (q * q * -0.000_013_079_099_5_f32 + q * 0.012_892_405_f32 - 0.007_483_88_f32)
        .round() as i32;
    let uv_f2 = (q * q * 0.000_003_265_178_3_f32 + q * 0.000_355_201_83_f32 + 0.002_280_92_f32)
        .round() as i32;

    // "Clamp to AV1 limits" (enc_cdef.c:891-895).
    let y_f1 = y_f1.clamp(0, 15);
    let y_f2 = y_f2.clamp(0, 3);
    let uv_f1 = uv_f1.clamp(0, 15);
    let uv_f2 = uv_f2.clamp(0, 3);

    CdefFrameParams {
        damping: (3 + (qindex >> 6)) as u8,
        y_strength: (y_f1 * 4 + y_f2) as u8,
        uv_strength: (uv_f1 * 4 + uv_f2) as u8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C-verified anchors (values cross-checked bit-exact against the C
    /// float evaluation for ALL qindexes by
    /// tests/c_parity_cdef_pick.rs; these pin representative points):
    /// q(255): ac=1828 -> y = 63 (pri 15, sec field 3), uv = 3 (uv_f1's
    /// quadratic goes negative and clamps to 0 — C's own fit);
    /// q(128): ac=176 -> y = 9, uv = 8; q(30): ac=37 -> all zero.
    #[test]
    fn strength_formula_anchors() {
        let p255 = pick_cdef_params_key_frame(255);
        assert_eq!(
            (p255.damping, p255.y_strength, p255.uv_strength),
            (6, 63, 3)
        );
        let p128 = pick_cdef_params_key_frame(128);
        assert_eq!(
            (p128.damping, p128.y_strength, p128.uv_strength),
            (5, 9, 8)
        );
        // Very low q: everything zero (CDEF off near-lossless).
        let p30 = pick_cdef_params_key_frame(30);
        assert_eq!((p30.y_strength, p30.uv_strength), (0, 0));
        assert_eq!(p30.damping, 3);
        assert!(!p30.any(true) && !p30.any(false));
    }

    /// The full recon-parity matrix qindexes {80,128,172,220,255} must all
    /// produce nonzero luma strengths (non-vacuous gate coverage; C-verified
    /// values y = 4/9/17/43/63) and legal field ranges everywhere.
    #[test]
    fn firing_profile_and_ranges() {
        for q in 0..=255u16 {
            let p = pick_cdef_params_key_frame(q as u8);
            assert!((3..=6).contains(&p.damping), "damping range at {q}");
            assert!(p.y_strength <= 63 && p.uv_strength <= 63);
        }
        // Zero below the knee (near-lossless protection)...
        assert_eq!(pick_cdef_params_key_frame(50).y_strength, 0);
        // ...firing across the entire gate matrix.
        for (q, want_y) in [(80u8, 4u8), (128, 9), (172, 17), (220, 43), (255, 63)] {
            assert_eq!(
                pick_cdef_params_key_frame(q).y_strength,
                want_y,
                "luma CDEF strength at qindex {q}"
            );
        }
        assert!(pick_cdef_params_key_frame(80).uv_strength != 0);
    }

    /// Damping steps exactly at the C breakpoints.
    #[test]
    fn damping_from_qp_breakpoints() {
        assert_eq!(pick_cdef_params_key_frame(0).damping, 3);
        assert_eq!(pick_cdef_params_key_frame(63).damping, 3);
        assert_eq!(pick_cdef_params_key_frame(64).damping, 4);
        assert_eq!(pick_cdef_params_key_frame(127).damping, 4);
        assert_eq!(pick_cdef_params_key_frame(128).damping, 5);
        assert_eq!(pick_cdef_params_key_frame(191).damping, 5);
        assert_eq!(pick_cdef_params_key_frame(192).damping, 6);
    }
}
