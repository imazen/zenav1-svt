//! Generates `svtav1-dsp/src/quant_tables.rs` from the C reference
//! (`svt_aom_dc_quant_qtx` / `svt_aom_ac_quant_qtx`, 8-bit).
//!
//! Usage:
//!   cargo run --release -p zenav1-svt-cref --bin gen_quant_tables \
//!     > crates/svtav1-dsp/src/quant_tables.rs

use svtav1_cref as cref;

fn main() {
    let mut out = String::new();
    out.push_str(
        "//! AV1 quantizer step tables extracted from the C reference\n\
         //! (svt_aom_dc_quant_qtx / svt_aom_ac_quant_qtx, 8-bit, delta 0).\n\
         //!\n\
         //! GENERATED FILE — DO NOT EDIT. Regenerate with:\n\
         //!   cargo run --release -p zenav1-svt-cref --bin gen_quant_tables \\\n\
         //!     > crates/svtav1-dsp/src/quant_tables.rs\n\n",
    );
    for (name, f) in [
        ("DC_QLOOKUP_8", cref::dc_quant_qtx as fn(i32) -> i16),
        ("AC_QLOOKUP_8", cref::ac_quant_qtx as fn(i32) -> i16),
    ] {
        out.push_str(&format!(
            "#[rustfmt::skip]\npub static {name}: [i16; 256] = [\n    "
        ));
        for q in 0..256 {
            out.push_str(&format!("{}, ", f(q)));
            if q % 16 == 15 {
                out.push_str("\n    ");
            }
        }
        out.push_str("\n];\n\n");
    }
    print!("{out}");
}
