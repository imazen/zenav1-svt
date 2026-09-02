//! FFI bindings for the CDF-CONTINUATION oracle
//! (`Source/Lib/Codec/cabac_context_model.c`).
//!
//! Backed by `shims/frame_cdf_shims.c`, which drives the REAL exported C
//! symbols `svt_aom_init_mode_probs`, `svt_av1_default_coef_probs` and
//! `svt_av1_reset_cdf_symbol_counters` — evidence tier 1
//! (`docs/WORKING-ON-THIS.md` §4).
//!
//! The shim answers ONE NAMED FIELD at a time rather than handing back a
//! struct, because the port does not share C's `FRAME_CONTEXT` layout (its
//! tables are split across three Rust types). See the shim's header comment.

use std::ffi::{CStr, CString, c_char};

/// What the shim puts in the `FRAME_CONTEXT` before reading a field out.
///
/// [`Self::Painted`] / [`Self::PaintedReset`] exist because a DEFAULT context
/// already has every adaptation counter at 0 — so `Defaults` and
/// `DefaultsReset` are identical, and a test built on that pair would still
/// pass with the counter reset deleted. Painting first makes the reset's
/// (nsymbs, stride) map for every field observable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum FctxMode {
    /// C's `PRIMARY_REF_NONE` arm: `svt_av1_default_coef_probs(qindex)` then
    /// `svt_aom_init_mode_probs` (`md_config_process.c:307-309`).
    Defaults = 0,
    /// [`Self::Defaults`], then `svt_av1_reset_cdf_symbol_counters`.
    DefaultsReset = 1,
    /// Every `AomCdfProb` painted `0x1212`.
    Painted = 2,
    /// [`Self::Painted`], then `svt_av1_reset_cdf_symbol_counters`.
    PaintedReset = 3,
}

unsafe extern "C" {
    fn ref_frame_ctx_field(
        qindex: i32,
        mode: i32,
        name: *const c_char,
        out: *mut u16,
        cap: usize,
    ) -> usize;
    fn ref_frame_ctx_field_count() -> usize;
    fn ref_frame_ctx_field_name(idx: usize) -> *const c_char;
}

/// C's `FRAME_CONTEXT.<name>` in one of four states — see [`FctxMode`].
///
/// Returns `None` when `name` is not a `FRAME_CONTEXT` field — which is a
/// meaningful answer, not an error: it is how a test learns that a name the
/// port invented has no C counterpart.
///
/// # Panics
///
/// Panics if C reports a longer field than the buffer this function allocated
/// for it — that would mean the two sides disagree about the field's size and
/// a silent prefix comparison would hide it.
pub fn frame_ctx_field(qindex: i32, mode: FctxMode, name: &str) -> Option<Vec<u16>> {
    let c_name = CString::new(name).ok()?;
    // Big enough for the largest FRAME_CONTEXT field (`coeff_base_cdf`, 2100).
    let mut buf = vec![0u16; 4096];
    let n = unsafe {
        ref_frame_ctx_field(
            qindex,
            mode as i32,
            c_name.as_ptr(),
            buf.as_mut_ptr(),
            buf.len(),
        )
    };
    if n == 0 {
        return None;
    }
    assert!(
        n <= buf.len(),
        "C's {name} has {n} elements, more than the {} this binding staged",
        buf.len()
    );
    buf.truncate(n);
    Some(buf)
}

/// Every `FRAME_CONTEXT` field name the shim knows, in C's declaration order.
///
/// Exists so a test can check the PORT's list against C's and name the
/// difference, instead of only walking the port's list and never discovering
/// what it omits.
pub fn frame_ctx_field_names() -> Vec<String> {
    let n = unsafe { ref_frame_ctx_field_count() };
    (0..n)
        .filter_map(|i| {
            let p = unsafe { ref_frame_ctx_field_name(i) };
            if p.is_null() {
                None
            } else {
                Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
            }
        })
        .collect()
}
