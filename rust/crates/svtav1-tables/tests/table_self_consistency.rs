//! Self-consistency pins for the block/transform geometry tables.
//!
//! WHY THIS FILE EXISTS. `svtav1-tables` had no tests at all, and four of its
//! tables have zero call sites anywhere in the workspace
//! (`BLOCK_SIZE_WIDE_LOG2`, `BLOCK_SIZE_HIGH_LOG2`, `PARTITION_CONTEXT_LOOKUP`,
//! `TX_TYPE_TO_1D`) — they are faithful transcriptions kept per the repo's
//! "dead-looking C stays translated" rule. A transcription typo in any of them
//! is the worst kind of defect this port can have: it is silent. Nothing
//! asserts, nothing panics; the encoder just codes a slightly wrong symbol and
//! the byte-identity gates go red somewhere far away — or, for the unused
//! tables, stay green until the day someone wires them up.
//!
//! WHAT IT CHECKS. Not "the table equals this other copy of the table" — that
//! only proves two transcriptions were made the same way. Each property below
//! derives one side MECHANICALLY from something independent:
//!
//! - the block/transform DIMENSIONS are re-derived from the enum VARIANT NAME
//!   (`BlockSize::Block16x32` must be 16 wide and 32 high). The name and the
//!   table were transcribed separately; agreeing is evidence.
//! - the LOG2 tables are re-derived by taking an actual base-2 logarithm.
//! - the 4x4-unit counts are re-derived by dividing by 4.
//! - `PARTITION_CONTEXT_LOOKUP` is re-derived from libaom's closed form
//!   (`av1/common/onyxc_int.h` `partition_context_lookup`): the value halves the
//!   remaining bits at each size step, i.e. `32 - (1 << (log2_dim - 2))`, taken
//!   on the WIDTH for `above` and the HEIGHT for `left`.
//! - `TX_TYPE_TO_1D` is re-derived from the 2D type's own name, and cross-checked
//!   against `TxType::is_2d()` — which is implemented as a discriminant
//!   comparison and therefore knows nothing about the table.
//!
//! These are total (every variant, no sampling) and pure (no encoder state, no
//! C library, no fixture), so they run in microseconds and can never flake.

use svtav1_tables::block::{
    BLOCK_SIZE_HIGH, BLOCK_SIZE_HIGH_LOG2, BLOCK_SIZE_WIDE, BLOCK_SIZE_WIDE_LOG2,
    NUM_4X4_BLOCKS_HIGH, NUM_4X4_BLOCKS_WIDE, PARTITION_CONTEXT_LOOKUP,
};
use svtav1_tables::transform::{
    TX_SIZE_HIGH, TX_SIZE_HIGH_LOG2, TX_SIZE_WIDE, TX_SIZE_WIDE_LOG2, TX_TYPE_TO_1D,
};
use svtav1_types::block::BlockSize;
use svtav1_types::transform::{TxSize, TxType, TxType1D};

/// Parse `WxH` out of a `Debug` name like `Block16x32` / `Tx4x16`.
fn dims_from_name(name: &str, prefix: &str) -> (u32, u32) {
    let body = name
        .strip_prefix(prefix)
        .unwrap_or_else(|| panic!("{name:?} does not start with {prefix:?}"));
    let (w, h) = body
        .split_once('x')
        .unwrap_or_else(|| panic!("{name:?} is not <prefix>WxH"));
    (
        w.parse()
            .unwrap_or_else(|e| panic!("{name:?}: width {w:?}: {e}")),
        h.parse()
            .unwrap_or_else(|e| panic!("{name:?}: height {h:?}: {e}")),
    )
}

fn log2_exact(v: u32, what: &str) -> u8 {
    assert!(
        v.is_power_of_two(),
        "{what}: {v} is not a power of two, so it has no exact log2"
    );
    v.trailing_zeros() as u8
}

#[test]
fn block_dimension_tables_agree_with_the_variant_names() {
    for b in BlockSize::ALL {
        let i = b.as_index();
        let name = format!("{b:?}");
        let (w, h) = dims_from_name(&name, "Block");
        assert_eq!(
            u32::from(BLOCK_SIZE_WIDE[i]),
            w,
            "BLOCK_SIZE_WIDE[{i}] disagrees with the variant name {name}"
        );
        assert_eq!(
            u32::from(BLOCK_SIZE_HIGH[i]),
            h,
            "BLOCK_SIZE_HIGH[{i}] disagrees with the variant name {name}"
        );
    }
}

#[test]
fn block_log2_and_4x4_count_tables_are_derivable_from_the_dimensions() {
    for b in BlockSize::ALL {
        let i = b.as_index();
        let name = format!("{b:?}");
        let (w, h) = (u32::from(BLOCK_SIZE_WIDE[i]), u32::from(BLOCK_SIZE_HIGH[i]));

        assert_eq!(
            BLOCK_SIZE_WIDE_LOG2[i],
            log2_exact(w, &name),
            "BLOCK_SIZE_WIDE_LOG2[{i}] ({name}) is not log2({w})"
        );
        assert_eq!(
            BLOCK_SIZE_HIGH_LOG2[i],
            log2_exact(h, &name),
            "BLOCK_SIZE_HIGH_LOG2[{i}] ({name}) is not log2({h})"
        );

        // A 4x4 unit is the AV1 mode-info granule; every legal block size is a
        // whole number of them in both directions.
        assert_eq!(w % 4, 0, "{name}: width {w} is not a multiple of 4");
        assert_eq!(h % 4, 0, "{name}: height {h} is not a multiple of 4");
        assert_eq!(
            u32::from(NUM_4X4_BLOCKS_WIDE[i]),
            w / 4,
            "NUM_4X4_BLOCKS_WIDE[{i}] ({name}) is not {w}/4"
        );
        assert_eq!(
            u32::from(NUM_4X4_BLOCKS_HIGH[i]),
            h / 4,
            "NUM_4X4_BLOCKS_HIGH[{i}] ({name}) is not {h}/4"
        );
    }
}

#[test]
fn partition_context_lookup_matches_the_libaom_closed_form() {
    // libaom builds this table so that each size step consumes half the
    // remaining context bits: 4->31, 8->30, 16->28, 32->24, 64->16, 128->0.
    // That is exactly `32 - (1 << (log2_dim - 2))`. `above` keys on the block
    // WIDTH, `left` on the HEIGHT — the asymmetry is the whole point of the
    // table, so a transposed pair would be invisible without this check.
    fn ctx(log2_dim: u8) -> i8 {
        (32i32 - (1i32 << (log2_dim - 2))) as i8
    }
    for b in BlockSize::ALL {
        let i = b.as_index();
        let name = format!("{b:?}");
        let (above, left) = PARTITION_CONTEXT_LOOKUP[i];
        assert_eq!(
            above,
            ctx(BLOCK_SIZE_WIDE_LOG2[i]),
            "PARTITION_CONTEXT_LOOKUP[{i}].above ({name}) is not the width-keyed closed form"
        );
        assert_eq!(
            left,
            ctx(BLOCK_SIZE_HIGH_LOG2[i]),
            "PARTITION_CONTEXT_LOOKUP[{i}].left ({name}) is not the height-keyed closed form"
        );
    }
}

#[test]
fn tx_dimension_tables_agree_with_the_variant_names_and_their_log2s() {
    for raw in 0..TxSize::SIZES_ALL as u8 {
        let t = TxSize::from_u8(raw).expect("0..SIZES_ALL are all valid TxSize discriminants");
        let i = t.as_index();
        let name = format!("{t:?}");
        let (w, h) = dims_from_name(&name, "Tx");
        assert_eq!(
            u32::from(TX_SIZE_WIDE[i]),
            w,
            "TX_SIZE_WIDE[{i}] disagrees with the variant name {name}"
        );
        assert_eq!(
            u32::from(TX_SIZE_HIGH[i]),
            h,
            "TX_SIZE_HIGH[{i}] disagrees with the variant name {name}"
        );
        assert_eq!(
            TX_SIZE_WIDE_LOG2[i],
            log2_exact(w, &name),
            "TX_SIZE_WIDE_LOG2[{i}] ({name}) is not log2({w})"
        );
        assert_eq!(
            TX_SIZE_HIGH_LOG2[i],
            log2_exact(h, &name),
            "TX_SIZE_HIGH_LOG2[{i}] ({name}) is not log2({h})"
        );
    }
}

#[test]
fn tx_type_to_1d_matches_the_2d_type_names_and_is_2d() {
    // The AV1 2D type names encode their own decomposition:
    //   <COL><ROW>  e.g. ADST_DCT = column ADST, row DCT
    //   IDTX        = identity in both directions
    //   V_<K>       = K down the columns, identity across the rows
    //   H_<K>       = identity down the columns, K across the rows
    use TxType1D::{Adst, Dct, FlipAdst, Identity};
    let expected: [(TxType, (TxType1D, TxType1D)); TxType::COUNT] = [
        (TxType::DctDct, (Dct, Dct)),
        (TxType::AdstDct, (Adst, Dct)),
        (TxType::DctAdst, (Dct, Adst)),
        (TxType::AdstAdst, (Adst, Adst)),
        (TxType::FlipAdstDct, (FlipAdst, Dct)),
        (TxType::DctFlipAdst, (Dct, FlipAdst)),
        (TxType::FlipAdstFlipAdst, (FlipAdst, FlipAdst)),
        (TxType::AdstFlipAdst, (Adst, FlipAdst)),
        (TxType::FlipAdstAdst, (FlipAdst, Adst)),
        (TxType::Idtx, (Identity, Identity)),
        (TxType::VDct, (Dct, Identity)),
        (TxType::HDct, (Identity, Dct)),
        (TxType::VAdst, (Adst, Identity)),
        (TxType::HAdst, (Identity, Adst)),
        (TxType::VFlipAdst, (FlipAdst, Identity)),
        (TxType::HFlipAdst, (Identity, FlipAdst)),
    ];
    for (t, want) in expected {
        let i = t as usize;
        assert_eq!(
            TX_TYPE_TO_1D[i], want,
            "TX_TYPE_TO_1D[{i}] disagrees with what the name {t:?} says it decomposes into"
        );
        // `is_2d()` is a discriminant comparison — it knows nothing about the
        // table, so this ties the two representations together. "2D" means a
        // real transform in BOTH directions.
        let (col, row) = TX_TYPE_TO_1D[i];
        assert_eq!(
            t.is_2d(),
            col != Identity && row != Identity,
            "{t:?}: is_2d() disagrees with its 1D decomposition {:?}",
            TX_TYPE_TO_1D[i]
        );
    }
}
