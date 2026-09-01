//! Differential tests for the compound / interintra syntax predicates of
//! `write_modes_b` steps 7 and 9
//! (`svtav1_encoder::port_entropy_inter::compound`).
//!
//! **Evidence tiers, stated per group** (`docs/WORKING-ON-THIS.md` §4):
//!
//! * **Tier 1** — `svt_aom_is_masked_compound_type`,
//!   `svt_aom_get_wedge_params_bits`, `svt_aom_get_wedge_bits_lookup` and
//!   `svt_aom_is_interintra_wedge_used` are EXPORTED, so the right-hand side
//!   of those assertions is a call into the release archive's own code.
//! * **Tier 1-header** — `is_comp_ref_allowed`,
//!   `is_interinter_compound_used` and `is_any_masked_compound_used` are
//!   `static INLINE` in `inter_prediction.h`. There is no symbol to call, so
//!   the shim compiles the header's source text: the C SOURCE, but not the
//!   archive's compiled copy of it. Stronger than a transcription, weaker
//!   than a call into the archive, and it is labelled that way rather than
//!   claimed as tier 1.
//!
//! The two WRITERS (`write_interintra_info`, `write_compound_type_info`) have
//! no differential here at all: they live inside `static write_modes_b`,
//! which no shim can reach, so their branch structure is tier 4 (hand-derived
//! vectors traced against the C source) in the module's own unit tests.
//! Comparing them against a second transcription and calling that
//! verification is exactly what §4 forbids.

use svtav1_cref::entropy_block as cref;
use svtav1_encoder::port_entropy_inter::compound as p;
use svtav1_encoder::port_entropy_inter::refframe::is_comp_ref_allowed;
use svtav1_encoder::port_md_rate_estimation::get_wedge_params_bits;
use svtav1_types::block::BlockSize;

/// Every `BlockSize` the port can express, with its C enum ordinal.
fn all_block_sizes() -> Vec<(BlockSize, i32)> {
    (0..22u8)
        .filter_map(|i| BlockSize::from_u8(i).map(|b| (b, i as i32)))
        .collect()
}

/// The four `CompoundType` values, port-side and as C ordinals.
fn all_compound_types() -> [(p::CompoundType, i32); p::COMPOUND_TYPES] {
    [
        (p::CompoundType::Average, 0),
        (p::CompoundType::Distwtd, 1),
        (p::CompoundType::Wedge, 2),
        (p::CompoundType::Diffwtd, 3),
    ]
}

/// Tier 1 — `svt_aom_is_masked_compound_type` is exported.
#[test]
fn c_parity_is_masked_compound_type() {
    for (t, ord) in all_compound_types() {
        assert_eq!(
            p::is_masked_compound_type(t),
            cref::is_masked_compound_type(ord),
            "compound type {t:?} (C ordinal {ord})",
        );
    }
    // The sentinel `COMPOUND_TYPES` is not a value a block can hold, so the
    // port has no variant for it; C still answers, and answers 0.
    assert!(!cref::is_masked_compound_type(p::COMPOUND_TYPES as i32));
}

/// Tier 1 — `svt_aom_get_wedge_params_bits` /
/// `svt_aom_get_wedge_bits_lookup` are exported, and this pins the table the
/// port keeps in `port_md_rate_estimation::WEDGE_PARAMS_BITS`.
#[test]
fn c_parity_wedge_params_bits_table() {
    let mut nonzero = 0;
    for (b, ord) in all_block_sizes() {
        let c = cref::wedge_params_bits(ord);
        assert_eq!(
            get_wedge_params_bits(b.as_index()),
            c,
            "wedge_params_bits at bsize {ord}",
        );
        // Upstream exposes the same value under two exported names; a port
        // that matched one and not the other would be half-right.
        assert_eq!(cref::wedge_bits_lookup(ord), c, "bits_lookup at {ord}");
        if c != 0 {
            nonzero += 1;
        }
    }
    // Anti-vacuity: the table is not uniformly zero, so the assertions above
    // discriminate. The count itself is a C measurement, not a port claim.
    assert!(
        nonzero > 0,
        "no bsize carries a wedge codebook — probe dead"
    );
}

/// Tier 1 — `svt_aom_is_interintra_wedge_used` is exported. It is the gate
/// `write_interintra_info` uses for the `wedge_interintra` sub-tree, and the
/// port spells it as `get_wedge_params_bits(...) > 0`.
#[test]
fn c_parity_is_interintra_wedge_used() {
    for (b, ord) in all_block_sizes() {
        assert_eq!(
            get_wedge_params_bits(b.as_index()) > 0,
            cref::is_interintra_wedge_used(ord),
            "is_interintra_wedge_used at bsize {ord}",
        );
    }
}

/// Tier 1-header — `is_comp_ref_allowed` is `static INLINE`.
#[test]
fn c_parity_is_comp_ref_allowed() {
    for (b, ord) in all_block_sizes() {
        assert_eq!(
            is_comp_ref_allowed(b),
            cref::is_comp_ref_allowed(ord),
            "is_comp_ref_allowed at bsize {ord}",
        );
    }
}

/// Tier 1-header — `is_interinter_compound_used`, over the full
/// `BlockSize` x `CompoundType` grid.
#[test]
fn c_parity_is_interinter_compound_used() {
    let mut trues = 0;
    let mut falses = 0;
    for (b, ord) in all_block_sizes() {
        for (t, tord) in all_compound_types() {
            let got = p::is_interinter_compound_used(t, b);
            assert_eq!(
                got,
                cref::is_interinter_compound_used(tord, ord),
                "is_interinter_compound_used({t:?}, bsize {ord})",
            );
            if got { trues += 1 } else { falses += 1 }
        }
    }
    // Anti-vacuity: the grid contains both answers, so an all-true or
    // all-false port would fail rather than pass.
    assert!(trues > 0 && falses > 0, "grid is one-sided — probe dead");
}

/// Tier 1-header — `is_any_masked_compound_used`.
#[test]
fn c_parity_is_any_masked_compound_used() {
    let mut trues = 0;
    for (b, ord) in all_block_sizes() {
        let got = p::is_any_masked_compound_used(b);
        assert_eq!(
            got,
            cref::is_any_masked_compound_used(ord),
            "is_any_masked_compound_used at bsize {ord}",
        );
        if got {
            trues += 1;
        }
    }
    assert!(trues > 0, "no bsize admits a masked compound — probe dead");
}
