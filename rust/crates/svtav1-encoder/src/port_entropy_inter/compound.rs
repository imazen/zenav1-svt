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

/// C `InterIntraMode` (definitions.h:1257).
///
/// An enum rather than C's bare `int`: the value indexes
/// `interintra_mode_cdf`, whose width is exactly these four.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum InterIntraMode {
    /// C `II_DC_PRED`.
    #[default]
    DcPred = 0,
    /// C `II_V_PRED`.
    VPred = 1,
    /// C `II_H_PRED`.
    HPred = 2,
    /// C `II_SMOOTH_PRED`.
    SmoothPred = 3,
}

/// C `CompoundType` (definitions.h:1259-1266).
///
/// C's enum also carries the sentinel `COMPOUND_TYPES = 4`; that is a count,
/// not a value a block can hold, so it is [`COMPOUND_TYPES`] here instead of a
/// variant nothing may construct.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum CompoundType {
    /// C `COMPOUND_AVERAGE`.
    #[default]
    Average = 0,
    /// C `COMPOUND_DISTWTD`.
    Distwtd = 1,
    /// C `COMPOUND_WEDGE`.
    Wedge = 2,
    /// C `COMPOUND_DIFFWTD`.
    Diffwtd = 3,
}

/// C `COMPOUND_TYPES` (definitions.h:1264) — the sentinel C's
/// `is_any_masked_compound_used` loop bounds itself by.
pub const COMPOUND_TYPES: usize = 4;

impl CompoundType {
    /// Every value the C loop in `is_any_masked_compound_used` visits, in C's
    /// order.
    const ALL: [CompoundType; COMPOUND_TYPES] = [
        CompoundType::Average,
        CompoundType::Distwtd,
        CompoundType::Wedge,
        CompoundType::Diffwtd,
    ];
}

/// C `svt_aom_is_masked_compound_type` (inter_prediction.c:34, EXPORTED).
#[inline]
pub const fn is_masked_compound_type(t: CompoundType) -> bool {
    matches!(t, CompoundType::Wedge | CompoundType::Diffwtd)
}

/// C `is_interinter_compound_used` (inter_prediction.h:288).
///
/// C's `default:` arm asserts and returns 0; that arm is unreachable for a
/// `CompoundType` value, and the exhaustive `match` here is what makes it so.
#[inline]
pub fn is_interinter_compound_used(t: CompoundType, bsize: BlockSize) -> bool {
    let comp_allowed = is_comp_ref_allowed(bsize);
    match t {
        CompoundType::Average | CompoundType::Distwtd | CompoundType::Diffwtd => comp_allowed,
        CompoundType::Wedge => comp_allowed && get_wedge_params_bits(bsize.as_index()) > 0,
    }
}

/// C `is_any_masked_compound_used` (inter_prediction.h:303).
///
/// C walks all four `CompoundType`s and filters with
/// [`is_masked_compound_type`]; the filter admits exactly `{Wedge, Diffwtd}`,
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
mod tests {
    use super::*;

    /// The nine wedge-carrying sizes are exactly the ones
    /// `is_interinter_compound_used(WEDGE, ·)` admits; every other size
    /// admits the three unmasked types iff `min(w,h) >= 8`.
    #[test]
    fn wedge_availability_tracks_the_codebook_table() {
        for i in 0..22u8 {
            let Some(b) = BlockSize::from_u8(i) else {
                continue;
            };
            let wedge = is_interinter_compound_used(CompoundType::Wedge, b);
            assert_eq!(
                wedge,
                is_comp_ref_allowed(b) && get_wedge_params_bits(b.as_index()) > 0,
                "bsize {i}"
            );
            for t in [
                CompoundType::Average,
                CompoundType::Distwtd,
                CompoundType::Diffwtd,
            ] {
                assert_eq!(
                    is_interinter_compound_used(t, b),
                    is_comp_ref_allowed(b),
                    "bsize {i} type {t:?}"
                );
            }
            // DIFFWTD is masked and needs only comp_ref_allowed, so
            // any-masked is exactly comp_ref_allowed.
            assert_eq!(is_any_masked_compound_used(b), is_comp_ref_allowed(b));
        }
    }

    #[test]
    fn masked_types_are_wedge_and_diffwtd_only() {
        assert!(!is_masked_compound_type(CompoundType::Average));
        assert!(!is_masked_compound_type(CompoundType::Distwtd));
        assert!(is_masked_compound_type(CompoundType::Wedge));
        assert!(is_masked_compound_type(CompoundType::Diffwtd));
    }

    /// Step 7's gate is a hard early-out: neither the symbol NOR the
    /// `rf[1] = INTRA_FRAME` assignment happens when it fails. Getting that
    /// backwards would suppress step 8 on every block of a sequence that
    /// disables interintra.
    #[test]
    fn interintra_gate_suppresses_the_ref_frame_mutation_too() {
        let mut ic = InterCdfs::new_default();
        let mut w = AomWriter::new(64);
        let mut rf = [1i8, -1];
        let used = write_interintra_info(
            &mut w,
            &mut ic,
            BlockSize::Block16x16,
            &mut rf,
            false,
            true,
            Some(InterIntraInfo {
                mode: InterIntraMode::VPred,
                use_wedge: false,
                wedge_index: 0,
            }),
        );
        assert!(!used);
        assert_eq!(rf, [1, -1], "no mutation behind a closed gate");
        assert_eq!(w.bytes_written(), 0, "no symbol behind a closed gate");
    }

    /// An interintra block sets `rf[1]` to INTRA_FRAME, which is what makes
    /// `write_modes_b` skip step 8 (`rf[1] != INTRA_FRAME`).
    #[test]
    fn interintra_sets_ref_frame_one_to_intra() {
        let mut ic = InterCdfs::new_default();
        let mut w = AomWriter::new(64);
        let mut rf = [1i8, -1];
        let used = write_interintra_info(
            &mut w,
            &mut ic,
            BlockSize::Block16x16,
            &mut rf,
            true,
            true,
            Some(InterIntraInfo {
                mode: InterIntraMode::SmoothPred,
                use_wedge: true,
                wedge_index: 9,
            }),
        );
        assert!(used);
        assert_eq!(rf[1], INTRA_FRAME);
    }

    /// Tier 4, traced against entropy_coding.c:5245-5272: a 16x16 interintra
    /// block with a wedge emits FOUR symbols (flag, mode, wedge flag, index)
    /// and a non-wedge one emits TWO. The count is what a decoder desync
    /// hinges on, so it is asserted through the op count rather than bytes.
    #[test]
    fn interintra_symbol_counts_match_the_c_branch_structure() {
        fn ops(use_wedge: bool, bsize: BlockSize) -> usize {
            let mut ic = InterCdfs::new_default();
            let mut w = AomWriter::new(64);
            let mut rf = [1i8, -1];
            let before = cdf_fingerprint(&ic);
            write_interintra_info(
                &mut w,
                &mut ic,
                bsize,
                &mut rf,
                true,
                true,
                Some(InterIntraInfo {
                    mode: InterIntraMode::HPred,
                    use_wedge,
                    wedge_index: 3,
                }),
            );
            let after = cdf_fingerprint(&ic);
            before.iter().zip(after).filter(|(a, b)| **a != *b).count()
        }
        // 16x16 has a wedge codebook: flag + mode + wedge-flag [+ index].
        assert_eq!(ops(false, BlockSize::Block16x16), 3);
        assert_eq!(ops(true, BlockSize::Block16x16), 4);
        // 4x8 has none, and is not interintra-allowed either, but the
        // wedge sub-gate alone drops the last two symbols: flag + mode.
        assert_eq!(ops(false, BlockSize::Block64x64), 2);
    }

    /// Each adapted CDF row is one written symbol; the fingerprint is the
    /// first element of every row this module can touch.
    fn cdf_fingerprint(ic: &InterCdfs) -> alloc::vec::Vec<u16> {
        let mut v = alloc::vec::Vec::new();
        v.extend(ic.interintra_cdf.iter().map(|r| r[0]));
        v.extend(ic.interintra_mode_cdf.iter().map(|r| r[0]));
        v.extend(ic.wedge_interintra_cdf.iter().map(|r| r[0]));
        v.extend(ic.wedge_idx_cdf.iter().map(|r| r[0]));
        v.extend(ic.comp_group_idx_cdf.iter().map(|r| r[0]));
        v.extend(ic.compound_index_cdf.iter().map(|r| r[0]));
        v.extend(ic.compound_type_cdf.iter().map(|r| r[0]));
        v
    }

    /// Tier 4, traced against entropy_coding.c:5279-5342. Group A with
    /// `enable_jnt_comp` off writes NOTHING beyond the group symbol; group B
    /// on a wedge-capable size writes type + index (+ a raw sign bit, which
    /// does not touch a CDF).
    #[test]
    fn compound_type_symbol_counts_match_the_c_branch_structure() {
        fn adapted(bsize: BlockSize, jnt: bool, group: CompGroup) -> usize {
            let mut ic = InterCdfs::new_default();
            let mut w = AomWriter::new(64);
            let before = cdf_fingerprint(&ic);
            write_compound_type_info(&mut w, &mut ic, bsize, true, jnt, 0, 0, group);
            let after = cdf_fingerprint(&ic);
            before.iter().zip(after).filter(|(a, b)| **a != *b).count()
        }
        // group symbol only.
        assert_eq!(
            adapted(
                BlockSize::Block16x16,
                false,
                CompGroup::A { compound_idx: true }
            ),
            1
        );
        // group symbol + compound_idx.
        assert_eq!(
            adapted(
                BlockSize::Block16x16,
                true,
                CompGroup::A { compound_idx: true }
            ),
            2
        );
        // group symbol + compound_type + wedge_idx.
        assert_eq!(
            adapted(
                BlockSize::Block16x16,
                true,
                CompGroup::B(InterInterComp {
                    comp_type: CompoundType::Wedge,
                    wedge_index: 5,
                    wedge_sign: true,
                    mask_type: 0,
                })
            ),
            3
        );
        // 64x64 has NO wedge codebook, so the compound_type symbol is
        // skipped and DIFFWTD is implicit: group symbol only, then a raw
        // literal that adapts nothing.
        assert_eq!(
            adapted(
                BlockSize::Block64x64,
                true,
                CompGroup::B(InterInterComp {
                    comp_type: CompoundType::Diffwtd,
                    wedge_index: 0,
                    wedge_sign: false,
                    mask_type: 1,
                })
            ),
            1
        );
    }
}
