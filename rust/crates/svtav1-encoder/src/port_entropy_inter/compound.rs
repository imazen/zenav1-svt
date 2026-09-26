//! Steps 7 and 9 of `write_modes_b`'s inter branch: the **interintra** syntax
//! group and the **compound-type** syntax group.
//!
//! C reference: `Source/Lib/Codec/entropy_coding.c:5245-5343` — two inline
//! blocks inside `write_modes_b` with no C function name of their own, plus
//! the three predicates they read (`svt_aom_is_masked_compound_type`,
//! inter_prediction.c:34; `is_interinter_compound_used` and
//! `is_any_masked_compound_used`, inter_prediction.h:288/:303).
//!
//! [`super::inter_mv_code`]'s nine-step map named these as the two steps with
//! no port. They are the last symbol groups an inter block can emit, so a
//! missing one is not a mis-coded value: it is a symbol the decoder reads and
//! the encoder never wrote, i.e. a desynced tile from that block onward.
//!
//! # The cross-step mutation in step 7
//!
//! C's step 7 does more than write. When `is_interintra_used` is set it
//! assigns `rf[1] = INTRA_FRAME` **into the block's own `ref_frame`**
//! (entropy_coding.c:5246-5249), and step 8's gate is
//! `frm_hdr->is_motion_mode_switchable && rf[1] != INTRA_FRAME`. So an
//! interintra block SUPPRESSES the motion-mode symbol, and it does so through
//! a side effect three lines earlier. [`write_interintra_info`] therefore
//! takes `ref_frame` by `&mut` rather than by value — the mutation is part of
//! the contract, not an implementation detail of C's struct plumbing.
//!
//! # Evidence
//!
//! Tier 1 for the three predicates and the two contexts they pair with:
//! `svt_aom_is_masked_compound_type` and `svt_aom_get_wedge_params_bits` are
//! exported symbols, and `tests/c_parity_entropy_compound.rs` drives them
//! through the `entropy_block` shim over every `BlockSize` x `CompoundType`.
//! `is_interinter_compound_used` / `is_any_masked_compound_used` are `static
//! INLINE` in `inter_prediction.h`; the shim compiles that header text, so the
//! differential drives the C SOURCE but not the release archive's copy of it —
//! labelled tier 1-header in the test, one notch below a call into the archive.
//!
//! The two writers themselves are tier 4 (hand-derived vectors traced against
//! the C source): they are built out of the tier-1-gated predicates above plus
//! `aom_write_symbol` calls, and `write_modes_b` is `static`, so no shim can
//! reach them. See `docs/WORKING-ON-THIS.md` §4.
//!
//! # Reachability
//!
//! Nothing here is called yet — the public entry point still refuses inter
//! frames (`pipeline.rs`, the `if !is_key` guard). Per §7 a faithful
//! translation with no caller stays translated.

use crate::entropy::writer::AomWriter;
use crate::port_entropy_inter::InterCdfs;
use crate::port_entropy_inter::modes::SIZE_GROUP_LOOKUP;
use crate::port_entropy_inter::refframe::{INTRA_FRAME, is_comp_ref_allowed};
use crate::port_md_rate_estimation::get_wedge_params_bits;
use svtav1_types::block::BlockSize;

/// C `INTERINTRA_MODES` (definitions.h:1257) — the `interintra_mode`
/// alphabet.
pub const INTERINTRA_MODES: usize = 4;

/// C `MASKED_COMPOUND_TYPES` (definitions.h:1265) — the `compound_type`
/// alphabet, `{COMPOUND_WEDGE, COMPOUND_DIFFWTD}`.
pub const MASKED_COMPOUND_TYPES: usize = 2;

/// C `MAX_DIFFWTD_MASK_BITS` (definitions.h:1294) — the literal width of the
/// diffwtd `mask_type`.
pub const MAX_DIFFWTD_MASK_BITS: u32 = 1;

/// C `MAX_WEDGE_TYPES` (definitions.h:1279) — the `wedge_index` alphabet.
pub const MAX_WEDGE_TYPES: usize = 16;

/// C `InterIntraMode` — unified: the single definition lives in `svtav1_types::prediction`.
pub use svtav1_types::prediction::InterIntraMode;

/// C `CompoundType` — unified: the single definition lives in `svtav1_types::prediction`.
pub use svtav1_types::prediction::CompoundType;

/// C `COMPOUND_TYPES` (definitions.h:1264) — the sentinel C's
/// `is_any_masked_compound_used` loop bounds itself by.
pub const COMPOUND_TYPES: usize = 4;

/// C `svt_aom_is_masked_compound_type` (inter_prediction.c:34, EXPORTED).
#[inline]
pub const fn is_masked_compound_type(t: CompoundType) -> bool {
    matches!(t, CompoundType::Wedge | CompoundType::DiffWtd)
}

/// C `is_interinter_compound_used` (inter_prediction.h:288).
///
/// C's `default:` arm asserts and returns 0; that arm is unreachable for a
/// `CompoundType` value, and the exhaustive `match` here is what makes it so.
#[inline]
pub fn is_interinter_compound_used(t: CompoundType, bsize: BlockSize) -> bool {
    let comp_allowed = is_comp_ref_allowed(bsize);
    match t {
        CompoundType::Average | CompoundType::DistWtd | CompoundType::DiffWtd => comp_allowed,
        CompoundType::Wedge => comp_allowed && get_wedge_params_bits(bsize.as_index()) > 0,
    }
}

/// C `is_any_masked_compound_used` (inter_prediction.h:303).
///
/// C walks all four `CompoundType`s and filters with
/// [`is_masked_compound_type`]; the filter admits exactly `{Wedge, DiffWtd}`,
/// so the iterator below visits the same set in the same order with the same
/// short-circuit. The leading `is_comp_ref_allowed` early-out is kept because
/// it is C's, even though every arm of [`is_interinter_compound_used`] would
/// re-derive it.
#[inline]
pub fn is_any_masked_compound_used(bsize: BlockSize) -> bool {
    if !is_comp_ref_allowed(bsize) {
        return false;
    }
    CompoundType::ALL
        .into_iter()
        .filter(|&t| is_masked_compound_type(t))
        .any(|t| is_interinter_compound_used(t, bsize))
}

/// The interintra syntax an inter block carries, when it carries any.
///
/// `None` at the call site means "this block is not interintra"; C spells the
/// same thing as `is_interintra_used == 0` plus three fields that are then
/// never read.
#[derive(Clone, Copy, Debug)]
pub struct InterIntraInfo {
    /// C `block_mi.interintra_mode`.
    pub mode: InterIntraMode,
    /// C `block_mi.use_wedge_interintra`.
    pub use_wedge: bool,
    /// C `block_mi.interintra_wedge_index`.
    pub wedge_index: u8,
}

/// C `write_modes_b` step 7 (entropy_coding.c:5245-5272) — the interintra
/// group, together with the `rf[1] = INTRA_FRAME` assignment that gates step
/// 8.
///
/// `enable_interintra_compound` is the SEQUENCE-header flag
/// (`scs->seq_header.enable_interintra_compound`); `allowed` is
/// [`super::modes::is_interintra_allowed`], which the caller has already
/// evaluated for its own reasons. Both are C's gate, kept as two parameters
/// so a caller cannot silently conflate them.
///
/// Returns `true` when the block is interintra, i.e. when `ref_frame[1]` was
/// set to `INTRA_FRAME` and step 8 must be skipped. The mutation happens
/// through `ref_frame` regardless, so a caller that re-reads `ref_frame[1]`
/// gets the same answer.
pub fn write_interintra_info(
    w: &mut AomWriter,
    ic: &mut InterCdfs,
    bsize: BlockSize,
    ref_frame: &mut [i8; 2],
    enable_interintra_compound: bool,
    allowed: bool,
    interintra: Option<InterIntraInfo>,
) -> bool {
    if !(enable_interintra_compound && allowed) {
        // C never reaches the assignment either: it is INSIDE the gate.
        return false;
    }
    if interintra.is_some() {
        ref_frame[1] = INTRA_FRAME;
    }
    let group = SIZE_GROUP_LOOKUP[bsize.as_index()] as usize;
    w.write_symbol(
        usize::from(interintra.is_some()),
        &mut ic.interintra_cdf[group],
        2,
    );
    let Some(ii) = interintra else {
        return false;
    };
    w.write_symbol(
        ii.mode as usize,
        &mut ic.interintra_mode_cdf[group],
        INTERINTRA_MODES,
    );
    // C `svt_aom_is_interintra_wedge_used` (inter_prediction.c:2015) — the
    // same `wedge_params_lookup[bsize].bits > 0` test the WEDGE arm of
    // `is_interinter_compound_used` makes, under a second upstream name.
    if get_wedge_params_bits(bsize.as_index()) > 0 {
        w.write_symbol(
            usize::from(ii.use_wedge),
            &mut ic.wedge_interintra_cdf[bsize.as_index()],
            2,
        );
        if ii.use_wedge {
            w.write_symbol(
                ii.wedge_index as usize,
                &mut ic.wedge_idx_cdf[bsize.as_index()],
                MAX_WEDGE_TYPES,
            );
        }
    }
    true
}

/// C `InterInterCompoundData`, cut to the three fields step 9 writes.
#[derive(Clone, Copy, Debug, Default)]
pub struct InterInterComp {
    /// C `interinter_comp.type`.
    pub comp_type: CompoundType,
    /// C `interinter_comp.wedge_index`.
    pub wedge_index: u8,
    /// C `interinter_comp.wedge_sign`.
    pub wedge_sign: bool,
    /// C `interinter_comp.mask_type` — a `DIFFWTD_MASK_TYPE`.
    pub mask_type: u8,
}

/// The two-valued choice C spells as `comp_group_idx` plus a pile of asserts.
///
/// Group A carries `compound_idx` (distance-weighted vs plain average); group
/// B carries a masked compound type. C codes `comp_group_idx` as a symbol
/// only when a masked type is available at this block size, and asserts it is
/// 0 otherwise — an enum makes the two cases distinguishable at the call site
/// instead of leaving a `u8` that can be silently out of range.
#[derive(Clone, Copy, Debug)]
pub enum CompGroup {
    /// C `comp_group_idx == 0`: `dist_wtd_comp` / `compound_average`.
    /// `compound_idx` is C's, and is coded only when `enable_jnt_comp`.
    A { compound_idx: bool },
    /// C `comp_group_idx == 1`: interintra / diffwtd / wedge.
    B(InterInterComp),
}

/// C `write_modes_b` step 9 (entropy_coding.c:5279-5342) — the compound-type
/// group, gated `has_second_ref(&mbmi->block_mi)` at the call site.
///
/// `comp_group_idx_ctx` is [`super::modes::comp_group_idx_context`] and
/// `comp_index_ctx` is [`super::modes::comp_index_context`]; both are
/// computed by the caller because their inputs (the neighbour pair, the
/// frame's order hints) are the block walk's, not this function's.
///
/// C's four `assert`s in this block are load-bearing documentation rather
/// than checks — they say group B implies a masked type is available, a
/// compound mode, and `SIMPLE_TRANSLATION`. Those are the caller's
/// invariants; violating them in C writes a stream no decoder accepts, so
/// they are `debug_assert!`s here for the same reason and with the same
/// force.
#[allow(clippy::too_many_arguments)]
pub fn write_compound_type_info(
    w: &mut AomWriter,
    ic: &mut InterCdfs,
    bsize: BlockSize,
    enable_masked_compound: bool,
    enable_jnt_comp: bool,
    comp_group_idx_ctx: usize,
    comp_index_ctx: usize,
    group: CompGroup,
) {
    let masked_compound_used = is_any_masked_compound_used(bsize) && enable_masked_compound;
    let group_idx = usize::from(matches!(group, CompGroup::B(_)));
    if masked_compound_used {
        w.write_symbol(group_idx, &mut ic.comp_group_idx_cdf[comp_group_idx_ctx], 2);
    } else {
        // C's `assert(mbmi->block_mi.comp_group_idx == 0)` — group B with no
        // masked type available is a stream the decoder cannot parse.
        debug_assert_eq!(group_idx, 0, "group B needs a masked compound type");
    }

    match group {
        CompGroup::A { compound_idx } => {
            if enable_jnt_comp {
                w.write_symbol(
                    usize::from(compound_idx),
                    &mut ic.compound_index_cdf[comp_index_ctx],
                    2,
                );
            } else {
                // C's `assert(mbmi->block_mi.compound_idx == 1)`.
                debug_assert!(compound_idx, "compound_idx is 1 without jnt_comp");
            }
        }
        CompGroup::B(comp) => {
            debug_assert!(masked_compound_used);
            debug_assert!(
                is_masked_compound_type(comp.comp_type),
                "group B carries COMPOUND_WEDGE or COMPOUND_DIFFWTD"
            );
            if is_interinter_compound_used(CompoundType::Wedge, bsize) {
                // C `interinter_comp.type - COMPOUND_WEDGE`: WEDGE -> 0,
                // DIFFWTD -> 1. When wedge is NOT usable at this size the
                // symbol is skipped entirely and the type is implicitly
                // DIFFWTD — that is C's, and it is why the write below is
                // NOT under the same gate.
                w.write_symbol(
                    comp.comp_type as usize - CompoundType::Wedge as usize,
                    &mut ic.compound_type_cdf[bsize.as_index()],
                    MASKED_COMPOUND_TYPES,
                );
            }
            if comp.comp_type == CompoundType::Wedge {
                debug_assert!(is_interinter_compound_used(CompoundType::Wedge, bsize));
                w.write_symbol(
                    comp.wedge_index as usize,
                    &mut ic.wedge_idx_cdf[bsize.as_index()],
                    MAX_WEDGE_TYPES,
                );
                w.write_bit(comp.wedge_sign);
            } else {
                w.write_literal(u32::from(comp.mask_type), MAX_DIFFWTD_MASK_BITS);
            }
        }
    }
}

#[cfg(test)]
mod tests;
