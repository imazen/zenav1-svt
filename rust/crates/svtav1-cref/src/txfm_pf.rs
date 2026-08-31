//! Reference bindings for the reduced-coefficient-shape (`_N2` / `_N4`)
//! forward transforms in `Codec/transforms.c`.
//!
//! Every symbol bound here is an exported symbol of `libSvtAv1Enc.a`
//! (verified with `nm -g`), so these drive the real C code — evidence tier 1
//! in `docs/WORKING-ON-THIS.md` §4. Nothing here keeps per-call state; all
//! scratch is a local `Vec` owned by the caller's stack frame, per the
//! threading rule at the top of `shims/ref_shims.c`.

/// `MAX_TXFM_STAGE_NUM` (inv_transforms.h). The `_c` kernels only read
/// `stage_range` inside `assert`s, but the pointer must still be valid.
const MAX_TXFM_STAGE_NUM: usize = 12;

macro_rules! decl_1d {
    ($($rust:ident => $c:ident),* $(,)?) => {
        unsafe extern "C" {
            $( fn $c(input: *const i32, output: *mut i32, cos_bit: i8, stage_range: *const i8); )*
        }
        $(
            /// Reference 1-D kernel. `output` is passed through unchanged
            /// where the C kernel does not write it (the caller sees the
            /// same partial writes the encoder does).
            pub fn $rust(input: &[i32], output: &mut [i32], cos_bit: i8) {
                let stage_range = [0i8; MAX_TXFM_STAGE_NUM];
                unsafe {
                    $c(
                        input.as_ptr(),
                        output.as_mut_ptr(),
                        cos_bit,
                        stage_range.as_ptr(),
                    )
                }
            }
        )*
    };
}

decl_1d! {
    fdct4_n2 => svt_av1_fdct4_new_N2,
    fdct8_n2 => svt_av1_fdct8_new_N2,
    fdct16_n2 => svt_av1_fdct16_new_N2,
    fdct32_n2 => svt_av1_fdct32_new_N2,
    fdct64_n2 => svt_av1_fdct64_new_N2,
    fdct4_n4 => svt_av1_fdct4_new_N4,
    fdct8_n4 => svt_av1_fdct8_new_N4,
    fdct16_n4 => svt_av1_fdct16_new_N4,
    fdct32_n4 => svt_av1_fdct32_new_N4,
    fdct64_n4 => svt_av1_fdct64_new_N4,
    fadst4_n2 => svt_av1_fadst4_new_N2,
    fadst8_n2 => svt_av1_fadst8_new_N2,
    fadst16_n2 => svt_av1_fadst16_new_N2,
    fadst4_n4 => svt_av1_fadst4_new_N4,
    fadst8_n4 => svt_av1_fadst8_new_N4,
    fadst16_n4 => svt_av1_fadst16_new_N4,
    fidentity4_n2 => svt_av1_fidentity4_N2_c,
    fidentity8_n2 => svt_av1_fidentity8_N2_c,
    fidentity16_n2 => svt_av1_fidentity16_N2_c,
    fidentity32_n2 => svt_av1_fidentity32_N2_c,
    fidentity4_n4 => svt_av1_fidentity4_N4_c,
    fidentity8_n4 => svt_av1_fidentity8_N4_c,
    fidentity16_n4 => svt_av1_fidentity16_N4_c,
    fidentity32_n4 => svt_av1_fidentity32_N4_c,
}
