//! Generates `svtav1-encoder/src/entropy/scan_tables.rs` from the C reference's
//! exported `eb_av1_scan_orders` / `tx_type_to_scan_index` data.
//!
//! Usage:
//!   cargo run --release -p zenav1-svt-cref --bin gen_scan_tables \
//!     > crates/svtav1-encoder/src/entropy/scan_tables.rs

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
         //!   cargo run --release -p zenav1-svt-cref --bin gen_scan_tables \\\n\
         //!     > crates/svtav1-encoder/src/entropy/scan_tables.rs\n\
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

    // Inverse scans (C `iscan`), computed at compile time from the tables
    // above: `iscan[pos]` is raster position `pos`'s index in the scan.
    out.push_str(
        "\n/// `invert(scan)[pos]` is the index of raster position `pos` in `scan`.\n\
         const fn invert<const N: usize>(scan: &[u16; N]) -> [u16; N] {\n\
         \x20   let mut out = [0u16; N];\n\
         \x20   let mut i = 0;\n\
         \x20   while i < N {\n\
         \x20       out[scan[i] as usize] = i as u16;\n\
         \x20       i += 1;\n\
         \x20   }\n\
         \x20   out\n\
         }\n\n",
    );
    for ts in 0..TX_SIZES_ALL {
        let len = cref::scan_len(ts);
        for class in 0..SCAN_CLASSES {
            out.push_str(&format!(
                "static ISCAN_TS{ts}_C{class}: [u16; {len}] = invert(&SCAN_TS{ts}_C{class});\n"
            ));
        }
    }
    out.push_str(
        "\n/// Inverse of [`scan`] (C `iscan`): the scan index of each raster position.\n\
         pub fn iscan(tx_size: usize, scan_class: usize) -> &'static [u16] {\n\
         \x20   const TABLE: [[&[u16]; 3]; 19] = [\n",
    );
    for ts in 0..TX_SIZES_ALL {
        out.push_str(&format!(
            "        [&ISCAN_TS{ts}_C0, &ISCAN_TS{ts}_C1, &ISCAN_TS{ts}_C2],\n"
        ));
    }
    out.push_str(
        "    ];\n\
         \x20   TABLE[tx_size][scan_class]\n\
         }\n",
    );

    // `iscan_for`: the inverse of a scan slice handed out by `scan`, found by
    // identity among the tables of the same length.
    let mut by_len: std::collections::BTreeMap<usize, Vec<(usize, usize)>> = Default::default();
    for ts in 0..TX_SIZES_ALL {
        for class in 0..SCAN_CLASSES {
            by_len.entry(cref::scan_len(ts)).or_default().push((ts, class));
        }
    }
    out.push_str(
        "\n/// The [`iscan`] table of a slice returned by [`scan`], found by identity;\n\
         /// `None` for any other slice.\n\
         pub fn iscan_for(scan: &[u16]) -> Option<&'static [u16]> {\n\
         \x20   let p = scan.as_ptr();\n\
         \x20   let tables: &[(&[u16], &'static [u16])] = match scan.len() {\n",
    );
    for (len, list) in &by_len {
        out.push_str(&format!("        {len} => &[\n"));
        for (ts, class) in list {
            out.push_str(&format!(
                "            (&SCAN_TS{ts}_C{class}, &ISCAN_TS{ts}_C{class}),\n"
            ));
        }
        out.push_str("        ],\n");
    }
    out.push_str(
        "        _ => return None,\n\
         \x20   };\n\
         \x20   tables\n\
         \x20       .iter()\n\
         \x20       .find(|(s, _)| core::ptr::eq(s.as_ptr(), p))\n\
         \x20       .map(|&(_, i)| i)\n\
         }\n",
    );

    print!("{out}");
}
