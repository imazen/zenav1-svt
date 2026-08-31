//! FFI bindings for the INTER BITSTREAM-SYNTAX oracle
//! (`Source/Lib/Codec/entropy_coding.c`, the inter group).
//!
//! Backed by `shims/entropy_inter_shims.c`, which drives the REAL exported C
//! symbols listed in that file's header comment — evidence tier 1
//! (`docs/WORKING-ON-THIS.md` §4).
//!
//! Kept in its own module (and its own C translation unit) so this lane never
//! shares an editable file with a concurrent lane.

/// Number of `i32`s describing one neighbour to the shim.
pub const NB_FIELDS: usize = 10;

/// Number of prediction contexts [`ref_contexts`] returns.
pub const N_CTX: usize = 19;

/// Number of CDF row indices [`cdf_rows`] returns.
pub const N_CDF_ROWS: usize = 16;

/// One spatial neighbour, in the exact field set C's context functions read.
///
/// `valid` and the caller's `up_available` / `left_available` are DISTINCT:
/// C's `av1_get_skip_mode_context`, `svt_aom_get_comp_index_context_enc` and
/// `svt_aom_get_comp_group_idx_context_enc` test the `xd->above_mbmi` /
/// `xd->left_mbmi` POINTER for null, while the ref-count, reference-mode and
/// comp-reference-type contexts test `up_available` / `left_available`.
#[derive(Clone, Copy, Debug, Default)]
pub struct NeighborDesc {
    pub valid: bool,
    pub mode: i32,
    pub ref_frame: [i32; 2],
    pub interp_filters: u32,
    pub use_intrabc: bool,
    pub skip_mode: bool,
    pub comp_group_idx: u8,
    pub compound_idx: u8,
    pub bsize: i32,
}

impl NeighborDesc {
    fn to_fields(self) -> [i32; NB_FIELDS] {
        [
            self.valid as i32,
            self.mode,
            self.ref_frame[0],
            self.ref_frame[1],
            self.interp_filters as i32,
            self.use_intrabc as i32,
            self.skip_mode as i32,
            self.comp_group_idx as i32,
            self.compound_idx as i32,
            self.bsize,
        ]
    }
}

unsafe extern "C" {
    fn ref_ec_collect_neighbors_ref_counts(
        above: *const i32,
        left: *const i32,
        up_avail: i32,
        left_avail: i32,
        out8: *mut u8,
    );
    fn ref_ec_ref_contexts(
        above: *const i32,
        left: *const i32,
        up_avail: i32,
        left_avail: i32,
        out: *mut i32,
    );
    fn ref_ec_cdf_rows(
        above: *const i32,
        left: *const i32,
        up_avail: i32,
        left_avail: i32,
        out: *mut i32,
    );
    fn ref_ec_comp_index_context(
        enable_order_hint: i32,
        order_hint_bits: i32,
        cur_frame_index: i32,
        bck_frame_index: i32,
        fwd_frame_index: i32,
        above: *const i32,
        left: *const i32,
    ) -> i32;
    fn ref_ec_switchable_interp_context(
        rf0: i32,
        rf1: i32,
        dir: i32,
        above: *const i32,
        left: *const i32,
        up_avail: i32,
        left_avail: i32,
    ) -> i32;
    fn ref_ec_is_nontrans_global_motion(
        mode: i32,
        bsize: i32,
        rf0: i32,
        rf1: i32,
        gm_wmtype: *const i32,
    ) -> i32;
    fn ref_ec_is_interintra_allowed(bsize: i32, mode: i32, rf0: i32, rf1: i32) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn ref_ec_motion_mode_allowed(
        is_motion_mode_switchable: i32,
        force_integer_mv: i32,
        allow_warped_motion: i32,
        gm_wmtype: *const i32,
        num_proj_ref: i32,
        overlappable_neighbors: i32,
        bsize: i32,
        rf0: i32,
        rf1: i32,
        mode: i32,
    ) -> i32;
    fn ref_ec_wb_signed_refsubexpfin(
        n: i32,
        k: i32,
        r: i32,
        v: i32,
        out: *mut u8,
        out_cap: i32,
    ) -> i32;
}

/// C `svt_aom_collect_neighbors_ref_counts_new` (entropy_coding.c:1877).
pub fn collect_neighbors_ref_counts(
    above: NeighborDesc,
    left: NeighborDesc,
    up_avail: bool,
    left_avail: bool,
) -> [u8; 8] {
    let (a, l) = (above.to_fields(), left.to_fields());
    let mut out = [0u8; 8];
    unsafe {
        ref_ec_collect_neighbors_ref_counts(
            a.as_ptr(),
            l.as_ptr(),
            up_avail as i32,
            left_avail as i32,
            out.as_mut_ptr(),
        )
    };
    out
}

/// Every ref-frame prediction context in one call; see the shim's comment for
/// the slot order (single_ref p1..p6, comp_ref p/p1/p2, comp_bwdref p/p1,
/// uni_comp_ref p/p1/p2, reference_mode, comp_reference_type, intra_inter,
/// skip_mode, comp_group_idx).
pub fn ref_contexts(
    above: NeighborDesc,
    left: NeighborDesc,
    up_avail: bool,
    left_avail: bool,
) -> [i32; N_CTX] {
    let (a, l) = (above.to_fields(), left.to_fields());
    let mut out = [0i32; N_CTX];
    unsafe {
        ref_ec_ref_contexts(
            a.as_ptr(),
            l.as_ptr(),
            up_avail as i32,
            left_avail as i32,
            out.as_mut_ptr(),
        )
    };
    out
}

/// The CDF SELECTORS as flat row indices into their tables — what
/// `WRITE_REF_BIT` actually dispatches on, so this pins the `[ctx][slot]`
/// indexing as well as the context derivation.
pub fn cdf_rows(
    above: NeighborDesc,
    left: NeighborDesc,
    up_avail: bool,
    left_avail: bool,
) -> [i32; N_CDF_ROWS] {
    let (a, l) = (above.to_fields(), left.to_fields());
    let mut out = [0i32; N_CDF_ROWS];
    unsafe {
        ref_ec_cdf_rows(
            a.as_ptr(),
            l.as_ptr(),
            up_avail as i32,
            left_avail as i32,
            out.as_mut_ptr(),
        )
    };
    out
}

/// C `svt_aom_get_comp_index_context_enc` (entropy_coding.c:52).
#[allow(clippy::too_many_arguments)]
pub fn comp_index_context(
    enable_order_hint: bool,
    order_hint_bits: i32,
    cur_frame_index: i32,
    bck_frame_index: i32,
    fwd_frame_index: i32,
    above: NeighborDesc,
    left: NeighborDesc,
) -> i32 {
    let (a, l) = (above.to_fields(), left.to_fields());
    unsafe {
        ref_ec_comp_index_context(
            enable_order_hint as i32,
            order_hint_bits,
            cur_frame_index,
            bck_frame_index,
            fwd_frame_index,
            a.as_ptr(),
            l.as_ptr(),
        )
    }
}

/// C `svt_aom_get_pred_context_switchable_interp` (entropy_coding.c:1527).
pub fn switchable_interp_context(
    rf0: i32,
    rf1: i32,
    dir: i32,
    above: NeighborDesc,
    left: NeighborDesc,
    up_avail: bool,
    left_avail: bool,
) -> i32 {
    let (a, l) = (above.to_fields(), left.to_fields());
    unsafe {
        ref_ec_switchable_interp_context(
            rf0,
            rf1,
            dir,
            a.as_ptr(),
            l.as_ptr(),
            up_avail as i32,
            left_avail as i32,
        )
    }
}

/// C `svt_aom_is_nontrans_global_motion` (entropy_coding.c:1572).
pub fn is_nontrans_global_motion(
    mode: i32,
    bsize: i32,
    rf0: i32,
    rf1: i32,
    gm_wmtype: &[i32; 8],
) -> bool {
    unsafe { ref_ec_is_nontrans_global_motion(mode, bsize, rf0, rf1, gm_wmtype.as_ptr()) != 0 }
}

/// C `svt_aom_is_interintra_allowed` (entropy_coding.c:4927).
pub fn is_interintra_allowed(bsize: i32, mode: i32, rf0: i32, rf1: i32) -> bool {
    unsafe { ref_ec_is_interintra_allowed(bsize, mode, rf0, rf1) != 0 }
}

/// C `svt_aom_motion_mode_allowed` (entropy_coding.c:1159), returning the
/// `MotionMode` discriminant (0 SIMPLE_TRANSLATION, 1 OBMC_CAUSAL,
/// 2 WARPED_CAUSAL).
#[allow(clippy::too_many_arguments)]
pub fn motion_mode_allowed(
    is_motion_mode_switchable: bool,
    force_integer_mv: bool,
    allow_warped_motion: bool,
    gm_wmtype: &[i32; 8],
    num_proj_ref: i32,
    overlappable_neighbors: i32,
    bsize: i32,
    rf0: i32,
    rf1: i32,
    mode: i32,
) -> i32 {
    unsafe {
        ref_ec_motion_mode_allowed(
            is_motion_mode_switchable as i32,
            force_integer_mv as i32,
            allow_warped_motion as i32,
            gm_wmtype.as_ptr(),
            num_proj_ref,
            overlappable_neighbors,
            bsize,
            rf0,
            rf1,
            mode,
        )
    }
}

/// C `svt_aom_wb_write_signed_primitive_refsubexpfin` (entropy_coding.c:2989).
/// Returns `(bits_written, bytes)`.
pub fn wb_signed_refsubexpfin(n: i32, k: i32, r: i32, v: i32) -> (usize, Vec<u8>) {
    const CAP: i32 = 64;
    let mut buf = vec![0u8; CAP as usize];
    let bits = unsafe { ref_ec_wb_signed_refsubexpfin(n, k, r, v, buf.as_mut_ptr(), CAP) };
    assert!(
        (0..=CAP * 8).contains(&bits),
        "bit count {bits} out of range"
    );
    let nbytes = (bits as usize).div_ceil(8);
    buf.truncate(nbytes);
    (bits as usize, buf)
}

// ---- default FRAME_CONTEXT tables this lane needs ----

macro_rules! ec_fc_tables {
    ($(($variant:ident, $sizeof_fn:ident, $copy_fn:ident)),* $(,)?) => {
        unsafe extern "C" {
            $(fn $sizeof_fn() -> usize;
              fn $copy_fn(dst: *mut u16);)*
        }

        /// Tables extractable from the C `FRAME_CONTEXT` after
        /// `svt_aom_init_mode_probs`.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum EcFcTable {
            $($variant,)*
        }

        /// Copy one default table out of the C context as a flat `u16` vector.
        pub fn ec_fc_table(t: EcFcTable) -> Vec<u16> {
            match t {
                $(EcFcTable::$variant => {
                    let bytes = unsafe { $sizeof_fn() };
                    assert!(bytes % 2 == 0);
                    let mut v = vec![0u16; bytes / 2];
                    unsafe { $copy_fn(v.as_mut_ptr()) };
                    v
                })*
            }
        }
    };
}

ec_fc_tables! {
    (CompRefType, ref_ec_sizeof_comp_ref_type_cdf, ref_ec_copy_comp_ref_type_cdf),
    (UniCompRef, ref_ec_sizeof_uni_comp_ref_cdf, ref_ec_copy_uni_comp_ref_cdf),
    (CompBwdRef, ref_ec_sizeof_comp_bwdref_cdf, ref_ec_copy_comp_bwdref_cdf),
    (SingleRef, ref_ec_sizeof_single_ref_cdf, ref_ec_copy_single_ref_cdf),
    (CompRef, ref_ec_sizeof_comp_ref_cdf, ref_ec_copy_comp_ref_cdf),
    (CompInter, ref_ec_sizeof_comp_inter_cdf, ref_ec_copy_comp_inter_cdf),
    (SkipMode, ref_ec_sizeof_skip_mode_cdfs, ref_ec_copy_skip_mode_cdfs),
    (NewMv, ref_ec_sizeof_newmv_cdf, ref_ec_copy_newmv_cdf),
    (ZeroMv, ref_ec_sizeof_zeromv_cdf, ref_ec_copy_zeromv_cdf),
    (RefMv, ref_ec_sizeof_refmv_cdf, ref_ec_copy_refmv_cdf),
    (Drl, ref_ec_sizeof_drl_cdf, ref_ec_copy_drl_cdf),
    (InterCompoundMode, ref_ec_sizeof_inter_compound_mode_cdf, ref_ec_copy_inter_compound_mode_cdf),
    (SwitchableInterp, ref_ec_sizeof_switchable_interp_cdf, ref_ec_copy_switchable_interp_cdf),
    (MotionMode, ref_ec_sizeof_motion_mode_cdf, ref_ec_copy_motion_mode_cdf),
    (Obmc, ref_ec_sizeof_obmc_cdf, ref_ec_copy_obmc_cdf),
    (CompoundIndex, ref_ec_sizeof_compound_index_cdf, ref_ec_copy_compound_index_cdf),
    (CompGroupIdx, ref_ec_sizeof_comp_group_idx_cdf, ref_ec_copy_comp_group_idx_cdf),
    (InterIntra, ref_ec_sizeof_interintra_cdf, ref_ec_copy_interintra_cdf),
    (InterIntraMode, ref_ec_sizeof_interintra_mode_cdf, ref_ec_copy_interintra_mode_cdf),
    (WedgeInterIntra, ref_ec_sizeof_wedge_interintra_cdf, ref_ec_copy_wedge_interintra_cdf),
    (WedgeIdx, ref_ec_sizeof_wedge_idx_cdf, ref_ec_copy_wedge_idx_cdf),
    (CompoundType, ref_ec_sizeof_compound_type_cdf, ref_ec_copy_compound_type_cdf),
}
