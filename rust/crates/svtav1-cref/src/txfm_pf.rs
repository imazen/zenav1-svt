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

// ---- transform config / stage range / 2-D entries / dispatch tables ----

/// Flattened `Txfm2dFlipCfg` length used by `ref_transform_config` and
/// `ref_get_inv_txfm_cfg` (see the shim for the field order).
pub const CFG_WORDS: usize = 11 + 2 * MAX_TXFM_STAGE_NUM;

unsafe extern "C" {
    fn ref_transform_config(tx_type: i32, tx_size: i32, out: *mut i32);
    fn ref_get_inv_txfm_cfg(tx_type: i32, tx_size: i32, out: *mut i32);
    fn ref_gen_fwd_stage_range(tx_type: i32, tx_size: i32, bd: i32, col: *mut i8, row: *mut i8);
    fn ref_fwd_txfm2d_pf(
        tx_size: i32,
        shape: i32,
        input: *mut i16,
        output: *mut i32,
        input_stride: u32,
        tx_type: i32,
        bd: u8,
    );
    fn ref_wht_fwd_txfm(
        src_diff: *mut i16,
        bw: i32,
        coeff: *mut i32,
        tx_size: i32,
        pf_shape: i32,
        bit_depth: i32,
        is_hbd: i32,
    );
    fn ref_highbd_fwd_txfm(
        variant: i32,
        src_diff: *mut i16,
        coeff: *mut i32,
        diff_stride: i32,
        tx_type: i32,
        tx_size: i32,
        bd: i32,
    );
    fn ref_handle_transform(which: i32, pf: i32, output: *mut i32) -> u64;
    fn ref_call_fwd_txfm_type_to_func(
        txfm_type: i32,
        input: *const i32,
        output: *mut i32,
        cos_bit: i8,
    ) -> i32;
    fn ref_call_inv_txfm_type_to_func(
        txfm_type: i32,
        input: *const i32,
        output: *mut i32,
        cos_bit: i8,
        stage_range_fill: i8,
    ) -> i32;
}

/// Reference `svt_aom_transform_config`, flattened (see [`CFG_WORDS`]).
pub fn transform_config(tx_type: usize, tx_size: usize) -> [i32; CFG_WORDS] {
    let mut out = [0i32; CFG_WORDS];
    unsafe { ref_transform_config(tx_type as i32, tx_size as i32, out.as_mut_ptr()) };
    out
}

/// Reference `svt_av1_get_inv_txfm_cfg`, flattened the same way.
pub fn get_inv_txfm_cfg(tx_type: usize, tx_size: usize) -> [i32; CFG_WORDS] {
    let mut out = [0i32; CFG_WORDS];
    unsafe { ref_get_inv_txfm_cfg(tx_type as i32, tx_size as i32, out.as_mut_ptr()) };
    out
}

/// Reference `svt_av1_gen_fwd_stage_range` for the config of
/// `(tx_type, tx_size)` at bit depth `bd`.
pub fn gen_fwd_stage_range(
    tx_type: usize,
    tx_size: usize,
    bd: i32,
) -> ([i8; MAX_TXFM_STAGE_NUM], [i8; MAX_TXFM_STAGE_NUM]) {
    let mut col = [0i8; MAX_TXFM_STAGE_NUM];
    let mut row = [0i8; MAX_TXFM_STAGE_NUM];
    unsafe {
        ref_gen_fwd_stage_range(
            tx_type as i32,
            tx_size as i32,
            bd,
            col.as_mut_ptr(),
            row.as_mut_ptr(),
        )
    };
    (col, row)
}

/// Reference reduced-shape 2-D forward transform. `shape` is C's
/// `TxCoeffShape` (1 = `N2_SHAPE`, 2 = `N4_SHAPE`).
///
/// `output` is passed in as-is so the caller can pre-fill it and observe
/// exactly which entries the C entry point writes.
#[allow(clippy::too_many_arguments)]
pub fn fwd_txfm2d_pf(
    tx_size: usize,
    shape: i32,
    input: &[i16],
    output: &mut [i32],
    input_stride: usize,
    tx_type: usize,
    bd: u8,
) {
    let mut inp = input.to_vec();
    unsafe {
        ref_fwd_txfm2d_pf(
            tx_size as i32,
            shape,
            inp.as_mut_ptr(),
            output.as_mut_ptr(),
            input_stride as u32,
            tx_type as i32,
            bd,
        )
    };
}

/// Reference `svt_av1_wht_fwd_txfm` (TPL's only transform entry).
pub fn wht_fwd_txfm(
    src_diff: &[i16],
    bw: usize,
    coeff: &mut [i32],
    tx_size: usize,
    pf_shape: i32,
    bit_depth: i32,
    is_hbd: bool,
) {
    let mut src = src_diff.to_vec();
    unsafe {
        ref_wht_fwd_txfm(
            src.as_mut_ptr(),
            bw as i32,
            coeff.as_mut_ptr(),
            tx_size as i32,
            pf_shape,
            bit_depth,
            i32::from(is_hbd),
        )
    };
}

/// Reference `svt_av1_highbd_fwd_txfm` (`variant` 0), `_n2` (1), `_n4` (2).
pub fn highbd_fwd_txfm(
    variant: i32,
    src_diff: &[i16],
    coeff: &mut [i32],
    diff_stride: usize,
    tx_type: usize,
    tx_size: usize,
    bd: i32,
) {
    let mut src = src_diff.to_vec();
    unsafe {
        ref_highbd_fwd_txfm(
            variant,
            src.as_mut_ptr(),
            coeff.as_mut_ptr(),
            diff_stride as i32,
            tx_type as i32,
            tx_size as i32,
            bd,
        )
    };
}

/// Reference `svt_handle_transform*`. `which`: 0=16x64, 1=32x64, 2=64x16,
/// 3=64x32, 4=64x64. `pf` selects the `_N2_N4_c` variant.
pub fn handle_transform(which: i32, pf: bool, output: &mut [i32]) -> u64 {
    unsafe { ref_handle_transform(which, i32::from(pf), output.as_mut_ptr()) }
}

/// Calls whatever `svt_aom_fwd_txfm_type_to_func(txfm_type)` returns.
/// Returns `false` (and leaves `output` untouched) when C returns NULL.
pub fn call_fwd_txfm_type_to_func(
    txfm_type: i32,
    input: &[i32],
    output: &mut [i32],
    cos_bit: i8,
) -> bool {
    unsafe {
        ref_call_fwd_txfm_type_to_func(txfm_type, input.as_ptr(), output.as_mut_ptr(), cos_bit) != 0
    }
}

/// Calls whatever `svt_aom_inv_txfm_type_to_func(txfm_type)` returns.
pub fn call_inv_txfm_type_to_func(
    txfm_type: i32,
    input: &[i32],
    output: &mut [i32],
    cos_bit: i8,
    stage_range_fill: i8,
) -> bool {
    unsafe {
        ref_call_inv_txfm_type_to_func(
            txfm_type,
            input.as_ptr(),
            output.as_mut_ptr(),
            cos_bit,
            stage_range_fill,
        ) != 0
    }
}

unsafe extern "C" {
    fn ref_fwd_txfm2d_default(
        tx_size: i32,
        input: *mut i16,
        output: *mut i32,
        input_stride: u32,
        tx_type: i32,
        bd: u8,
    );
}

/// Reference DEFAULT-shape 2-D forward transform — the `_c` implementations
/// (`svt_av1_transform_two_d_*_c` / `svt_av1_fwd_txfm2d_*_c`), NOT the RTCD
/// pointers, so this is the pure-C oracle.
pub fn fwd_txfm2d_default(
    tx_size: usize,
    input: &[i16],
    output: &mut [i32],
    input_stride: usize,
    tx_type: usize,
    bd: u8,
) {
    let mut inp = input.to_vec();
    unsafe {
        ref_fwd_txfm2d_default(
            tx_size as i32,
            inp.as_mut_ptr(),
            output.as_mut_ptr(),
            input_stride as u32,
            tx_type as i32,
            bd,
        )
    };
}
