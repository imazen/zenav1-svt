//! Generates `svtav1-entropy/src/scan_tables.rs` from the C reference's
//! exported `eb_av1_scan_orders` / `tx_type_to_scan_index` data.
//!
//! Usage:
//!   cargo run --release -p svtav1-cref --bin gen_scan_tables \
//!     > crates/svtav1-entropy/src/scan_tables.rs

use svtav1_cref as cref;

const TX_SIZES_ALL: usize = 19;
const SCAN_CLASSES: usize = 3;

fn main() {
    let mut out = String::new();
    out.push_str(
        "//! Scan orders extracted from the C reference (`eb_av1_scan_orders`,\n\
         //! libSvtAv1Enc.a, SVT-AV1 v4.1.0). Indexed by C `TxSize` value and the\n\
         //! scan class from `TX_TYPE_TO_SCAN_INDEX` (0 = default/2D, 1 = row\n\
         //! (vertical tx classes), 2 = column (horizontal tx classes)).\n\
         //!\n\
         //! GENERATED FILE — DO NOT EDIT. Regenerate with:\n\
         //!   cargo run --release -p svtav1-cref --bin gen_scan_tables \\\n\
         //!     > crates/svtav1-entropy/src/scan_tables.rs\n\
         //! The c_scan_tables_match test asserts these stay in sync with C.\n\n",
    );

    out.push_str(
        "/// C `tx_type_to_scan_index[TX_TYPES]`.\n\
         pub static TX_TYPE_TO_SCAN_INDEX: [u8; 16] = [",
    );
    for t in 0..16 {
        out.push_str(&format!("{}, ", cref::tx_type_to_scan_index(t)));
    }
    out.push_str("];\n\n");

    for ts in 0..TX_SIZES_ALL {
        let len = cref::scan_len(ts);
        for class in 0..SCAN_CLASSES {
            let scan = cref::scan(ts, class);
            assert_eq!(scan.len(), len);
            out.push_str(&format!(
                "#[rustfmt::skip]\nstatic SCAN_TS{ts}_C{class}: [u16; {len}] = [\n    "
            ));
            for (i, v) in scan.iter().enumerate() {
                assert!(*v >= 0);
                out.push_str(&format!("{v}, "));
                if i % 16 == 15 {
                    out.push_str("\n    ");
                }
            }
            out.push_str("\n];\n");
        }
    }

    out.push_str(
        "\n/// Scan order for (C TxSize value, scan class). Length is the\n\
         /// adjusted (64->32 capped) coefficient count for the transform size.\n\
         pub fn scan(tx_size: usize, scan_class: usize) -> &'static [u16] {\n\
         \x20   const TABLE: [[&[u16]; 3]; 19] = [\n",
    );
    for ts in 0..TX_SIZES_ALL {
        out.push_str(&format!(
            "        [&SCAN_TS{ts}_C0, &SCAN_TS{ts}_C1, &SCAN_TS{ts}_C2],\n"
        ));
    }
    out.push_str(
        "    ];\n\
         \x20   TABLE[tx_size][scan_class]\n\
         }\n",
    );

    print!("{out}");
}
