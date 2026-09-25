use super::*;

// ---- Inter MV entropy write + CDF adaptation oracle (chunk C3) ----

unsafe extern "C" {
    pub(super) fn ref_get_mv_joint(mv_x: i16, mv_y: i16) -> i32;
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ref_encode_mv_real_seq(
        mv_x: *const i16,
        mv_y: *const i16,
        ref_x: *const i16,
        ref_y: *const i16,
        n: i32,
        allow_high_precision_mv: i32,
        force_integer_mv: i32,
        allow_update_cdf: i32,
        nmvc_in: *const u16,
        nmvc_out: *mut u16,
        out: *mut u8,
        cap: u32,
    ) -> u32;
    pub(super) fn ref_reset_nmv_counter(nmvc_in: *const u16, nmvc_out: *mut u16);
}

/// Reference `svt_av1_get_mv_joint` (rd_cost.c:47) — the EXPORTED joint-type
/// classifier used by the MV *rate* side (`mv_cost`) and by the static
/// `av1_update_mv_stats`. Takes an already-differenced MV.
pub fn get_mv_joint(mv: (i16, i16)) -> i32 {
    unsafe { ref_get_mv_joint(mv.0, mv.1) }
}

/// Output of [`encode_mv_real_seq`]: the finalized od_ec bytes plus the
/// `NmvContext` as it stands AFTER the whole sequence adapted through it.
#[derive(Debug, Clone)]
pub struct EncodeMvSeqOut {
    /// Finalized range-coder bytes.
    pub bytes: Vec<u8>,
    /// Final adapted `NmvContext`, flat in C struct-layout order
    /// ([`NMV_FLAT_LEN`] u16s).
    pub nmvc: Vec<u16>,
}

/// Drive the REAL exported `svt_av1_encode_mv` (entropy_coding.c:1492) over a
/// sequence of `(mv, ref_mv)` pairs through ONE adapting `NmvContext`.
///
/// `nmvc_in` seeds the context (`None` = the C `default_nmv_context`).
/// `allow_high_precision_mv` is the `usehp` argument every C call site passes
/// (`frm_hdr->allow_high_precision_mv`); `force_integer_mv` is read by
/// `svt_av1_encode_mv` itself off `ppcs->frm_hdr` and overrides `usehp` to
/// `MV_SUBPEL_NONE`. `allow_update_cdf` is the `AomWriter` field that gates
/// `aom_write_symbol`'s in-place CDF adaptation.
pub fn encode_mv_real_seq(
    mvs: &[(i16, i16)],
    refs: &[(i16, i16)],
    allow_high_precision_mv: bool,
    force_integer_mv: bool,
    allow_update_cdf: bool,
    nmvc_in: Option<&[u16]>,
) -> EncodeMvSeqOut {
    assert_eq!(mvs.len(), refs.len());
    assert_eq!(unsafe { ref_nmv_context_flat_len() }, NMV_FLAT_LEN);
    if let Some(f) = nmvc_in {
        assert_eq!(f.len(), NMV_FLAT_LEN);
    }
    let mv_x: Vec<i16> = mvs.iter().map(|m| m.0).collect();
    let mv_y: Vec<i16> = mvs.iter().map(|m| m.1).collect();
    let ref_x: Vec<i16> = refs.iter().map(|m| m.0).collect();
    let ref_y: Vec<i16> = refs.iter().map(|m| m.1).collect();
    let mut bytes = vec![0u8; 1 << 16];
    let mut nmvc = vec![0u16; NMV_FLAT_LEN];
    let nbytes = unsafe {
        ref_encode_mv_real_seq(
            mv_x.as_ptr(),
            mv_y.as_ptr(),
            ref_x.as_ptr(),
            ref_y.as_ptr(),
            mvs.len() as i32,
            i32::from(allow_high_precision_mv),
            i32::from(force_integer_mv),
            i32::from(allow_update_cdf),
            nmvc_in.map_or(core::ptr::null(), |f| f.as_ptr()),
            nmvc.as_mut_ptr(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
        )
    };
    assert!(
        nbytes as usize <= bytes.len(),
        "MV seq exceeded oracle buffer"
    );
    bytes.truncate(nbytes as usize);
    EncodeMvSeqOut { bytes, nmvc }
}

/// Reference `reset_nmv_counter` (cabac_context_model.c:1956, `static`),
/// driven through its EXPORTED caller `svt_av1_reset_cdf_symbol_counters`
/// (:1971). Returns the flat `NmvContext` with every adaptation counter
/// zeroed.
pub fn reset_nmv_counter(nmvc_in: &[u16]) -> Vec<u16> {
    assert_eq!(nmvc_in.len(), NMV_FLAT_LEN);
    let mut out = vec![0u16; NMV_FLAT_LEN];
    unsafe { ref_reset_nmv_counter(nmvc_in.as_ptr(), out.as_mut_ptr()) };
    out
}

unsafe extern "C" {
    pub(super) fn ref_have_newmv_in_inter_mode(mode: i32) -> i32;
    pub(super) fn ref_have_nearmv_in_inter_mode(mode: i32) -> i32;
    pub(super) fn ref_is_inter_compound_mode(mode: i32) -> i32;
    pub(super) fn ref_is_inter_singleref_mode(mode: i32) -> i32;
}

/// Reference `svt_aom_have_newmv_in_inter_mode` (mode_decision.c:257, exported).
pub fn have_newmv_in_inter_mode(mode: u8) -> bool {
    unsafe { ref_have_newmv_in_inter_mode(i32::from(mode)) != 0 }
}
/// Reference `have_nearmv_in_inter_mode` (inter_prediction.h:416, static inline).
pub fn have_nearmv_in_inter_mode(mode: u8) -> bool {
    unsafe { ref_have_nearmv_in_inter_mode(i32::from(mode)) != 0 }
}
/// Reference `is_inter_compound_mode` (definitions.h:1622, static inline).
pub fn is_inter_compound_mode(mode: u8) -> bool {
    unsafe { ref_is_inter_compound_mode(i32::from(mode)) != 0 }
}
/// Reference `is_inter_singleref_mode` (definitions.h:1626, static inline).
pub fn is_inter_singleref_mode(mode: u8) -> bool {
    unsafe { ref_is_inter_singleref_mode(i32::from(mode)) != 0 }
}

unsafe extern "C" {
    pub(super) fn ref_update_cdf_level_default(enc_mode: i32, is_islice: i32, is_base: i32) -> i32;
    pub(super) fn ref_update_cdf_level_rtc(enc_mode: i32, is_islice: i32) -> i32;
    pub(super) fn ref_update_cdf_level_allintra(enc_mode: i32) -> i32;
}

/// Reference `svt_aom_get_update_cdf_level_default` (enc_mode_config.c:8510).
/// `is_islice` is the C `SliceType` (`B_SLICE = 0`, `I_SLICE = 1`).
pub fn update_cdf_level_default(enc_mode: i32, is_islice: bool, is_base: bool) -> u8 {
    unsafe {
        ref_update_cdf_level_default(enc_mode, i32::from(is_islice), i32::from(is_base)) as u8
    }
}
/// Reference `svt_aom_get_update_cdf_level_rtc` (enc_mode_config.c:8524).
pub fn update_cdf_level_rtc(enc_mode: i32, is_islice: bool) -> u8 {
    unsafe { ref_update_cdf_level_rtc(enc_mode, i32::from(is_islice)) as u8 }
}
/// Reference `svt_aom_get_update_cdf_level_allintra` (enc_mode_config.c:8534).
pub fn update_cdf_level_allintra(enc_mode: i32) -> u8 {
    unsafe { ref_update_cdf_level_allintra(enc_mode) as u8 }
}

// ---------------------------------------------------------------------------
