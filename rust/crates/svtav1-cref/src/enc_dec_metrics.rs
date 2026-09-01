//! Reference bindings for `Codec/enc_dec_process.c`'s SSIM walkers
//! (lane `wx-rc`).
//!
//! **Evidence tier 1 for ONE function.** `aom_ssim2` (:695) is `static` in C
//! but survives the Release build as a local symbol with its SOURCE ABI
//! intact, so `build.rs` promotes it with `llvm-objcopy --globalize-symbol`
//! and the differential drives the real compiled C. That one promotion pins
//! everything under it, all of which was inlined: `ssim_8x8`,
//! `svt_aom_ssim_parms_8x8_c` and `svt_aom_similarity`.
//!
//! **THREE neighbours are deliberately NOT bound**, each because LLVM
//! specialised its ABI: `aom_highbd_ssim2` (its `bd` and `shift` were
//! constant-folded — the only call site passes a literal `shift = 0`, and the
//! compiled body materialises the bd-10 constants as immediates), and
//! `avg_cdf_symbol` / `avg_cdf_symbols` (their weight parameters were
//! propagated out). See `link_globalized_enc_dec_statics` in `build.rs` for
//! the disassembly and the symptom each produced. The 10-bit SSIM path is
//! therefore evidence tier 4 in the port and says so.

/// Whether `build.rs` could promote `enc_dec_process.c`'s SSIM walkers on
/// this host.
///
/// The SKIP DECISION BELONGS TO THE CALLER (the project's no-silent-skip
/// rule): set `SVT_CREF_REQUIRE_ENC_DEC_STATICS=1` and
/// [`enc_dec_statics_oracle_is_available`] fails loudly instead.
pub const ENC_DEC_STATICS_AVAILABLE: bool = cfg!(enc_dec_statics);

/// Fail loudly when the caller demanded the tier-1 oracle and the host cannot
/// provide it.
///
/// # Panics
/// When `SVT_CREF_REQUIRE_ENC_DEC_STATICS` is set to a non-empty, non-`0`
/// value and the promotion did not happen.
pub fn enc_dec_statics_oracle_is_available() -> bool {
    if !ENC_DEC_STATICS_AVAILABLE {
        let required = std::env::var("SVT_CREF_REQUIRE_ENC_DEC_STATICS")
            .map(|v| !v.is_empty() && v != "0")
            .unwrap_or(false);
        assert!(
            !required,
            "SVT_CREF_REQUIRE_ENC_DEC_STATICS is set but build.rs could not promote \
             enc_dec_process.c's statics — see its cargo:warning."
        );
    }
    ENC_DEC_STATICS_AVAILABLE
}

#[cfg(enc_dec_statics)]
unsafe extern "C" {
    fn ref_aom_ssim2(
        img1: *const u8,
        stride_img1: i32,
        img2: *const u8,
        stride_img2: i32,
        width: i32,
        height: i32,
    ) -> f64;
}

/// Reference `aom_ssim2` (enc_dec_process.c:695).
///
/// `None` when the promotion is unavailable on this host.
///
/// # Panics
/// When a plane is shorter than `stride * height`, which would let C read past
/// the buffer.
#[must_use]
#[allow(unused_variables)]
pub fn ssim2(
    img1: &[u8],
    stride_img1: usize,
    img2: &[u8],
    stride_img2: usize,
    width: usize,
    height: usize,
) -> Option<f64> {
    assert!(img1.len() >= stride_img1 * height, "img1 too short for C");
    assert!(img2.len() >= stride_img2 * height, "img2 too short for C");
    #[cfg(enc_dec_statics)]
    {
        Some(unsafe {
            ref_aom_ssim2(
                img1.as_ptr(),
                stride_img1 as i32,
                img2.as_ptr(),
                stride_img2 as i32,
                width as i32,
                height as i32,
            )
        })
    }
    #[cfg(not(enc_dec_statics))]
    {
        None
    }
}
