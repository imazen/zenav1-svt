//! Inter-frame BITSTREAM SYNTAX — the inter group of
//! `Source/Lib/Codec/entropy_coding.c`, ported wholesale.
//!
//! This module owns the syntax elements an inter block and an inter frame
//! header emit: the intra/inter flag, the whole reference-frame signalling
//! tree and its contexts, the single-ref and compound inter mode symbols, the
//! DRL index, skip-mode, the motion-mode / interintra gates, the switchable
//! interpolation filter, and the frame header's global-motion parameters
//! (with the `aom_wb_*` bit-buffer primitive stack underneath them).
//!
//! # Coverage — what is here and what is NOT
//!
//! Every function of the inter group is ported. Two carry a caveat rather
//! than a gap, named rather than implied:
//!
//! * `write_frame_size_with_refs` (entropy_coding.c:3238) takes its two
//!   sub-writers (`write_superres_scale`, `write_frame_size`) as closures:
//!   both live outside this lane's queue and already have counterparts in
//!   `entropy/obu.rs`, and a second copy here would be a silently diverging
//!   one. See [`framesize`].
//! * `write_sgrproj_filter` (entropy_coding.c:4069) is here, but the
//!   `RESTORE_SWITCHABLE` frame-level plumbing that would reach it lives in
//!   `entropy/lr.rs`, which this lane does not own.
//!
//! # Reachability
//!
//! Nothing here is called yet: the public entry point still refuses inter
//! frames (`pipeline.rs`, the `if !is_key` guard) and the wiring belongs to
//! the chunks that own `pipeline.rs` / `entropy/obu.rs`. Per
//! `docs/WORKING-ON-THIS.md` §7 a faithful translation with no caller stays
//! translated; this note is here rather than an `#[allow(dead_code)]`.
//!
//! # Evidence
//!
//! `tests/c_parity_entropy_inter.rs` drives the REAL exported C symbols
//! through `svtav1-cref`'s `entropy_inter` shim — evidence tier 1
//! (`docs/WORKING-ON-THIS.md` §4) — for every context function, every CDF
//! selector, the motion-mode / interintra / non-translational-GM gates, the
//! switchable-interp context, the default CDF tables, and the header
//! bit-buffer `svt_aom_wb_write_signed_primitive_refsubexpfin`.
//!
//! The `static` C functions in this group (`write_is_inter`,
//! `write_ref_frames`, `write_inter_mode`, `write_drl_idx`,
//! `encode_intra_luma_mode_nonkey_av1`, `encode_skip_mode_av1`,
//! `write_motion_mode`, `av1_is_interp_needed`, `svt_aom_get_ref_filter_type`,
//! `write_mb_interp_filter`, `write_inter_compound_mode`,
//! `write_global_motion{,_params}`, the three `aom_wb_write_primitive_*`,
//! `write_sgrproj_filter`) are defined in `entropy_coding.c`, which
//! `shims/ref_shims.c` never compiles — a shim cannot reach them at all, so
//! tier 1 is structurally unavailable there. They are built ENTIRELY out of
//! the tier-1-gated pieces above (context + CDF selector + table), which is
//! where the derivation bugs live; the branch structure on top is covered at
//! tier 4 (hand-derived vectors traced against the C source) until an inter
//! byte gate exists. Writing a shim that re-transcribes their bit trees and
//! calling the agreement tier 1 is exactly what §4 forbids.

pub mod block;
pub mod cdfs;
pub mod compound;
pub mod framesize;
pub mod gm;
pub mod interp;
pub mod metadata;
pub mod modes;
pub mod neighbors;
pub mod primitives;
pub mod refframe;

use crate::entropy::cdf::AomCdfProb;
use refframe::{BWDREF_FRAME, INTRA_FRAME, TOTAL_REFS_PER_FRAME};

/// One spatial neighbour, holding exactly the `BlockModeInfo` fields the C
/// context functions in this module read.
#[derive(Clone, Copy, Debug, Default)]
pub struct NeighborMi {
    /// C `block_mi.mode` (a `PredictionMode` discriminant).
    pub mode: u8,
    /// C `block_mi.ref_frame`.
    pub ref_frame: [i8; 2],
    /// C `block_mi.interp_filters` — the packed pair
    /// `(y_filter) | (x_filter << 16)`.
    pub interp_filters: u32,
    /// C `block_mi.use_intrabc`.
    pub use_intrabc: bool,
    /// C `block_mi.skip_mode`.
    pub skip_mode: bool,
    /// C `block_mi.comp_group_idx`.
    pub comp_group_idx: u8,
    /// C `block_mi.compound_idx`.
    pub compound_idx: u8,
    /// C `MbModeInfo.bsize` (a `BlockSize` discriminant).
    pub bsize: u8,
}

impl NeighborMi {
    /// C `has_second_ref` (block_structures.h:105).
    #[inline]
    pub fn has_second_ref(&self) -> bool {
        self.ref_frame[1] > INTRA_FRAME
    }

    /// C `is_intrabc_block` (block_structures.h:113).
    #[inline]
    pub fn is_intrabc_block(&self) -> bool {
        self.use_intrabc
    }

    /// C `is_inter_block` (block_structures.h:117) — note IntraBC counts as
    /// inter here, which is why an IntraBC neighbour contributes an
    /// `INTRA_FRAME` (index 0) ref count.
    #[inline]
    pub fn is_inter_block(&self) -> bool {
        self.use_intrabc || self.ref_frame[0] > INTRA_FRAME
    }

    /// C `has_uni_comp_refs` (block_structures.h:109).
    #[inline]
    pub fn has_uni_comp_refs(&self) -> bool {
        self.has_second_ref()
            && !((self.ref_frame[0] >= BWDREF_FRAME) ^ (self.ref_frame[1] >= BWDREF_FRAME))
    }
}

/// The neighbour half of C's `MacroBlockD`.
///
/// The pointer and the availability flag are SEPARATE knobs because C reads
/// them separately: `av1_get_skip_mode_context` (entropy_coding.c:1097),
/// `svt_aom_get_comp_index_context_enc` (:52) and
/// `svt_aom_get_comp_group_idx_context_enc` (:80) test the `above_mbmi` /
/// `left_mbmi` POINTER for null, while
/// `svt_aom_collect_neighbors_ref_counts_new`, the reference-mode and
/// comp-reference-type contexts, `svt_av1_get_intra_inter_context` and
/// `svt_aom_get_pred_context_switchable_interp` test `up_available` /
/// `left_available`. Conflating the two is a silent context divergence.
///
/// C would dereference a null pointer if `up_available` were set with
/// `above == None`; that input is invalid rather than meaningful, and the
/// accessors below treat it as unavailable instead of panicking.
#[derive(Clone, Copy, Debug, Default)]
pub struct Neighbors {
    /// C `xd->above_mbmi` (`None` == the null pointer).
    pub above: Option<NeighborMi>,
    /// C `xd->left_mbmi`.
    pub left: Option<NeighborMi>,
    /// C `xd->up_available`.
    pub up_available: bool,
    /// C `xd->left_available`.
    pub left_available: bool,
}

impl Neighbors {
    /// The above neighbour as the `up_available`-gated sites see it.
    #[inline]
    pub fn above_avail(&self) -> Option<&NeighborMi> {
        if self.up_available {
            self.above.as_ref()
        } else {
            None
        }
    }

    /// The left neighbour as the `left_available`-gated sites see it.
    #[inline]
    pub fn left_avail(&self) -> Option<&NeighborMi> {
        if self.left_available {
            self.left.as_ref()
        } else {
            None
        }
    }
}

/// C `svt_av1_get_intra_inter_context` (entropy_coding.c:1127).
///
/// Already ported once in `entropy/context.rs`; repeated here only so the
/// tier-1 test in this lane can gate the whole context family through one
/// oracle call. The two must agree — if they ever do not, one of them is
/// wrong and this module's parity test is the one with a C oracle behind it.
pub fn intra_inter_context(nb: &Neighbors) -> usize {
    match (nb.above_avail(), nb.left_avail()) {
        (Some(a), Some(l)) => {
            let above_intra = !a.is_inter_block();
            let left_intra = !l.is_inter_block();
            if left_intra && above_intra {
                3
            } else {
                usize::from(left_intra || above_intra)
            }
        }
        (Some(e), None) | (None, Some(e)) => 2 * usize::from(!e.is_inter_block()),
        (None, None) => 0,
    }
}

/// The per-frame CDFs this lane needs that `FrameContext` either lacks
/// entirely or initialises to a UNIFORM PLACEHOLDER.
///
/// See [`cdfs`] for the full accounting of which is which and why the split
/// exists. This is mutable, per-frame state: the writers adapt it in place
/// exactly as `aom_write_symbol` adapts `FrameContext`.
#[derive(Clone, Debug)]
pub struct InterCdfs {
    /// C `comp_ref_type_cdf` — absent from `FrameContext`.
    pub comp_ref_type_cdf: [[AomCdfProb; 3]; 5],
    /// C `uni_comp_ref_cdf` — absent from `FrameContext`.
    pub uni_comp_ref_cdf: [[[AomCdfProb; 3]; 3]; 3],
    /// C `comp_bwdref_cdf` — absent from `FrameContext`.
    pub comp_bwdref_cdf: [[[AomCdfProb; 3]; 2]; 3],
    /// C `skip_mode_cdfs` — `FrameContext::skip_mode_cdf` is a placeholder.
    pub skip_mode_cdf: [[AomCdfProb; 3]; 3],
    /// C `newmv_cdf` — `FrameContext::newmv_cdf` is a placeholder.
    pub newmv_cdf: [[AomCdfProb; 3]; 6],
    /// C `zeromv_cdf` — `FrameContext::globalmv_cdf` is a placeholder.
    pub zeromv_cdf: [[AomCdfProb; 3]; 2],
    /// C `refmv_cdf` — `FrameContext::refmv_cdf` is a placeholder.
    pub refmv_cdf: [[AomCdfProb; 3]; 6],
    /// C `drl_cdf` — `FrameContext::drl_cdf` is a placeholder.
    pub drl_cdf: [[AomCdfProb; 3]; 3],
    /// C `inter_compound_mode_cdf` — `FrameContext`'s is a placeholder.
    pub inter_compound_mode_cdf: [[AomCdfProb; 9]; 8],
    /// C `switchable_interp_cdf` — `FrameContext::interp_filter_cdf` is a
    /// placeholder.
    pub switchable_interp_cdf: [[AomCdfProb; 4]; 16],
    /// C `motion_mode_cdf` — absent from `FrameContext`.
    pub motion_mode_cdf: [[AomCdfProb; 4]; 22],
    /// C `obmc_cdf` — absent from `FrameContext`.
    pub obmc_cdf: [[AomCdfProb; 3]; 22],
    /// C `compound_index_cdf` — absent from `FrameContext`.
    pub compound_index_cdf: [[AomCdfProb; 3]; 6],
    /// C `comp_group_idx_cdf` — absent from `FrameContext`.
    pub comp_group_idx_cdf: [[AomCdfProb; 3]; 6],
    /// C `interintra_cdf` — absent from `FrameContext`.
    pub interintra_cdf: [[AomCdfProb; 3]; 4],
    /// C `interintra_mode_cdf` — absent from `FrameContext`.
    pub interintra_mode_cdf: [[AomCdfProb; 5]; 4],
    /// C `wedge_interintra_cdf` — absent from `FrameContext`.
    pub wedge_interintra_cdf: [[AomCdfProb; 3]; 22],
    /// C `wedge_idx_cdf` — absent from `FrameContext`.
    pub wedge_idx_cdf: [[AomCdfProb; 17]; 22],
    /// C `compound_type_cdf` — absent from `FrameContext`.
    pub compound_type_cdf: [[AomCdfProb; 3]; 22],
}

impl Default for InterCdfs {
    fn default() -> Self {
        Self::new_default()
    }
}

impl InterCdfs {
    /// The C defaults, as `svt_aom_init_mode_probs` installs them.
    pub fn new_default() -> Self {
        Self {
            comp_ref_type_cdf: cdfs::COMP_REF_TYPE_CDF,
            uni_comp_ref_cdf: cdfs::UNI_COMP_REF_CDF,
            comp_bwdref_cdf: cdfs::COMP_BWDREF_CDF,
            skip_mode_cdf: cdfs::SKIP_MODE_CDF,
            newmv_cdf: cdfs::NEWMV_CDF,
            zeromv_cdf: cdfs::ZEROMV_CDF,
            refmv_cdf: cdfs::REFMV_CDF,
            drl_cdf: cdfs::DRL_CDF,
            inter_compound_mode_cdf: cdfs::INTER_COMPOUND_MODE_CDF,
            switchable_interp_cdf: cdfs::SWITCHABLE_INTERP_CDF,
            motion_mode_cdf: cdfs::MOTION_MODE_CDF,
            obmc_cdf: cdfs::OBMC_CDF,
            compound_index_cdf: cdfs::COMPOUND_INDEX_CDF,
            comp_group_idx_cdf: cdfs::COMP_GROUP_IDX_CDF,
            interintra_cdf: cdfs::INTERINTRA_CDF,
            interintra_mode_cdf: cdfs::INTERINTRA_MODE_CDF,
            wedge_interintra_cdf: cdfs::WEDGE_INTERINTRA_CDF,
            wedge_idx_cdf: cdfs::WEDGE_IDX_CDF,
            compound_type_cdf: cdfs::COMPOUND_TYPE_CDF,
        }
    }
}

/// Sanity: the ref-count array is `TOTAL_REFS_PER_FRAME` wide.
const _: () = assert!(TOTAL_REFS_PER_FRAME == 8);
