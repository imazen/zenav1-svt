//! Generates `svtav1-encoder/src/entropy/default_cdfs.rs` from the C reference.
//!
//! Extracts every default CDF table from libSvtAv1Enc.a's FRAME_CONTEXT
//! (via `svt_av1_default_coef_probs` + `svt_aom_init_mode_probs`) and emits
//! them as Rust statics in exact C layout and dimensions.
//!
//! Usage:
//!   cargo run --release -p zenav1-svt-cref --bin gen_default_cdfs \
//!     > crates/svtav1-encoder/src/entropy/default_cdfs.rs
//!
//! A drift test in svtav1-encoder (tests/c_parity_entropy.rs) re-extracts at test time and asserts the
//! committed tables match the linked C library bit for bit.

use svtav1_cref::{FcTable, fc_init, fc_table};

/// Representative base_qindex per TOKEN_CDF_Q_CTXS bucket
/// (C `get_q_ctx`: <=20 -> 0, <=60 -> 1, <=120 -> 2, else 3).
pub const Q_REPS: [i32; 4] = [10, 40, 100, 160];

struct Tbl {
    rust_name: &'static str,
    table: FcTable,
    /// Dimensions, outermost first; innermost includes the CDF_SIZE(+1) slot.
    dims: &'static [usize],
    /// Coefficient tables depend on the base_qindex bucket.
    q_dependent: bool,
}

const TABLES: &[Tbl] = &[
    // ---- coefficient CDFs (q-dependent, svt_av1_default_coef_probs) ----
    Tbl {
        rust_name: "TXB_SKIP_CDF",
        table: FcTable::TxbSkip,
        dims: &[5, 13, 3],
        q_dependent: true,
    },
    Tbl {
        rust_name: "EOB_EXTRA_CDF",
        table: FcTable::EobExtra,
        dims: &[5, 2, 9, 3],
        q_dependent: true,
    },
    Tbl {
        rust_name: "DC_SIGN_CDF",
        table: FcTable::DcSign,
        dims: &[2, 3, 3],
        q_dependent: true,
    },
    Tbl {
        rust_name: "EOB_FLAG_CDF16",
        table: FcTable::EobFlag16,
        dims: &[2, 2, 6],
        q_dependent: true,
    },
    Tbl {
        rust_name: "EOB_FLAG_CDF32",
        table: FcTable::EobFlag32,
        dims: &[2, 2, 7],
        q_dependent: true,
    },
    Tbl {
        rust_name: "EOB_FLAG_CDF64",
        table: FcTable::EobFlag64,
        dims: &[2, 2, 8],
        q_dependent: true,
    },
    Tbl {
        rust_name: "EOB_FLAG_CDF128",
        table: FcTable::EobFlag128,
        dims: &[2, 2, 9],
        q_dependent: true,
    },
    Tbl {
        rust_name: "EOB_FLAG_CDF256",
        table: FcTable::EobFlag256,
        dims: &[2, 2, 10],
        q_dependent: true,
    },
    Tbl {
        rust_name: "EOB_FLAG_CDF512",
        table: FcTable::EobFlag512,
        dims: &[2, 2, 11],
        q_dependent: true,
    },
    Tbl {
        rust_name: "EOB_FLAG_CDF1024",
        table: FcTable::EobFlag1024,
        dims: &[2, 2, 12],
        q_dependent: true,
    },
    Tbl {
        rust_name: "COEFF_BASE_EOB_CDF",
        table: FcTable::CoeffBaseEob,
        dims: &[5, 2, 4, 4],
        q_dependent: true,
    },
    Tbl {
        rust_name: "COEFF_BASE_CDF",
        table: FcTable::CoeffBase,
        dims: &[5, 2, 42, 5],
        q_dependent: true,
    },
    Tbl {
        rust_name: "COEFF_BR_CDF",
        table: FcTable::CoeffBr,
        dims: &[4, 2, 21, 5],
        q_dependent: true,
    },
    // ---- mode CDFs (q-independent, svt_aom_init_mode_probs) ----
    Tbl {
        rust_name: "PARTITION_CDF",
        table: FcTable::Partition,
        dims: &[20, 11],
        q_dependent: false,
    },
    Tbl {
        rust_name: "SKIP_CDF",
        table: FcTable::Skip,
        dims: &[3, 3],
        q_dependent: false,
    },
    Tbl {
        rust_name: "KF_Y_CDF",
        table: FcTable::KfY,
        dims: &[5, 5, 14],
        q_dependent: false,
    },
    Tbl {
        rust_name: "ANGLE_DELTA_CDF",
        table: FcTable::AngleDelta,
        dims: &[8, 8],
        q_dependent: false,
    },
    Tbl {
        rust_name: "INTRA_EXT_TX_CDF",
        table: FcTable::IntraExtTx,
        dims: &[3, 4, 13, 17],
        q_dependent: false,
    },
    Tbl {
        // C FRAME_CONTEXT.inter_ext_tx_cdf[EXT_TX_SETS_INTER][EXT_TX_SIZES]
        // [CDF_SIZE(TX_TYPES)] (default_inter_ext_tx_cdf,
        // cabac_context_model.c:316-333). Consumed by IntraBC blocks (the
        // only inter-classified blocks on this allintra port).
        rust_name: "INTER_EXT_TX_CDF",
        table: FcTable::InterExtTx,
        dims: &[4, 4, 17],
        q_dependent: false,
    },
    Tbl {
        // C FRAME_CONTEXT.txfm_partition_cdf[TXFM_PARTITION_CONTEXTS]
        // [CDF_SIZE(2)] (default_txfm_partition_cdf,
        // cabac_context_model.c:587-592) — the inter var-tx tx_size writer's
        // CDF rows (`write_tx_size_vartx`); IntraBC-only here.
        rust_name: "TXFM_PARTITION_CDF",
        table: FcTable::TxfmPartition,
        dims: &[21, 3],
        q_dependent: false,
    },
    Tbl {
        rust_name: "TX_SIZE_CDF",
        table: FcTable::TxSize,
        dims: &[4, 3, 4],
        q_dependent: false,
    },
    Tbl {
        rust_name: "UV_MODE_CDF",
        table: FcTable::UvMode,
        dims: &[2, 13, 15],
        q_dependent: false,
    },
    Tbl {
        rust_name: "FILTER_INTRA_CDF",
        table: FcTable::FilterIntra,
        dims: &[22, 3],
        q_dependent: false,
    },
    Tbl {
        rust_name: "FILTER_INTRA_MODE_CDF",
        table: FcTable::FilterIntraMode,
        dims: &[6],
        q_dependent: false,
    },
    Tbl {
        rust_name: "DELTA_Q_CDF",
        table: FcTable::DeltaQ,
        dims: &[5],
        q_dependent: false,
    },
    Tbl {
        rust_name: "INTRABC_CDF",
        table: FcTable::IntraBc,
        dims: &[3],
        q_dependent: false,
    },
    Tbl {
        rust_name: "PALETTE_Y_MODE_CDF",
        table: FcTable::PaletteYMode,
        dims: &[7, 3, 3],
        q_dependent: false,
    },
    Tbl {
        rust_name: "PALETTE_UV_MODE_CDF",
        table: FcTable::PaletteUvMode,
        dims: &[2, 3],
        q_dependent: false,
    },
    Tbl {
        rust_name: "PALETTE_Y_SIZE_CDF",
        table: FcTable::PaletteYSize,
        dims: &[7, 8],
        q_dependent: false,
    },
    Tbl {
        rust_name: "PALETTE_Y_COLOR_INDEX_CDF",
        table: FcTable::PaletteYColorIndex,
        dims: &[7, 5, 9],
        q_dependent: false,
    },
    Tbl {
        rust_name: "Y_MODE_CDF",
        table: FcTable::YMode,
        dims: &[4, 14],
        q_dependent: false,
    },
];

fn type_str(dims: &[usize]) -> String {
    let mut s = "AomCdfProb".to_string();
    for d in dims.iter().rev() {
        s = format!("[{s}; {d}]");
    }
    s
}

fn emit(out: &mut String, data: &[u16], dims: &[usize], indent: usize) {
    let pad = " ".repeat(indent);
    if dims.len() == 1 {
        let row: Vec<String> = data.iter().map(|v| v.to_string()).collect();
        out.push_str(&format!("{pad}[{}],\n", row.join(", ")));
        return;
    }
    let inner: usize = dims[1..].iter().product();
    out.push_str(&format!("{pad}[\n"));
    for chunk in data.chunks(inner) {
        emit(out, chunk, &dims[1..], indent + 4);
    }
    out.push_str(&format!("{pad}],\n"));
}

/// Doc comments carried into the generated tables (cited C source).
fn doc_for(name: &str) -> &'static str {
    match name {
        "PALETTE_Y_MODE_CDF" => {
            "/// C `default_palette_y_mode_cdf` (cabac_context_model.c:497):\n\
             /// \\[palette bsize ctx 0..6\\]\\[neighbor-palette-count ctx 0..2\\], 2 symbols.\n"
        }
        "PALETTE_UV_MODE_CDF" => {
            "/// C `default_palette_uv_mode_cdf` (cabac_context_model.c:507):\n\
             /// \\[y-palette-used ctx 0..1\\], 2 symbols.\n"
        }
        "PALETTE_Y_SIZE_CDF" => {
            "/// C `default_palette_y_size_cdf` (cabac_context_model.c:477):\n\
             /// \\[palette bsize ctx 0..6\\], 7 symbols (palette size 2..8).\n"
        }
        "PALETTE_Y_COLOR_INDEX_CDF" => {
            "/// C `default_palette_y_color_index_cdf` (cabac_context_model.c:511):\n\
             /// \\[palette_size - 2\\]\\[color ctx 0..4\\], n = size symbols per row\n\
             /// (count slot sits at index n — full-row averaging in the per-SB\n\
             /// chain is equivalent to C AVG_CDF_STRIDE because the tails are\n\
             /// structurally zero on both sides).\n"
        }
        _ => "",
    }
}

fn emit_table(out: &mut String, name: &str, data: &[u16], dims: &[usize]) {
    let total: usize = dims.iter().product();
    assert_eq!(
        data.len(),
        total,
        "{name}: C sizeof/2 = {}, expected dims product {} — dims are wrong",
        data.len(),
        total
    );
    out.push_str(doc_for(name));
    out.push_str(&format!(
        "#[rustfmt::skip]\npub static {name}: {} = ",
        type_str(dims)
    ));
    let mut body = String::new();
    emit(&mut body, data, dims, 0);
    // Strip the trailing ",\n" of the outermost emit and terminate.
    let body = body.trim_end().trim_end_matches(',');
    out.push_str(body);
    out.push_str(";\n\n");
}

/// Which file under `default_cdfs/` a table goes to. The coefficient tables
/// are per-q (4x) and large, so `COEFF_BASE_CDF` gets a file of its own; one
/// file for everything was 4,600 lines.
fn file_for(t: &Tbl) -> &'static str {
    match (t.q_dependent, t.rust_name) {
        (true, "COEFF_BASE_CDF") => "coeff_base",
        (true, _) => "coeff",
        (false, _) => "mode",
    }
}

const FILES: [&str; 3] = ["coeff", "coeff_base", "mode"];

fn main() {
    // Writes `<dir>/default_cdfs.rs` and `<dir>/default_cdfs/{coeff,
    // coeff_base, mode}.rs`; `<dir>` is `crates/svtav1-encoder/src/entropy`.
    let dir = std::env::args()
        .nth(1)
        .expect("usage: gen_default_cdfs <crates/svtav1-encoder/src/entropy>");
    let mut out = String::new();
    out.push_str(
        "//! Default CDF tables extracted from the C reference (libSvtAv1Enc.a,\n\
         //! SVT-AV1 v4.1.0) via `svt_av1_default_coef_probs` and\n\
         //! `svt_aom_init_mode_probs`. Exact C `FRAME_CONTEXT` layout: ICDF\n\
         //! values, structural 0 at `[nsymbs-1]`, adaptation counter slot at\n\
         //! `[nsymbs]`.\n\
         //!\n\
         //! GENERATED FILES — DO NOT EDIT. Regenerate with:\n\
         //!   cargo run --release -p zenav1-svt-cref --bin gen_default_cdfs -- \\\n\
         //!     crates/svtav1-encoder/src/entropy\n\
         //! The c_default_cdfs_match test asserts these stay in sync with C.\n\
         \n\
         use crate::entropy::cdf::AomCdfProb;\n\
         \n\
         mod coeff;\n\
         mod coeff_base;\n\
         mod mode;\n\
         pub use coeff::*;\n\
         pub use coeff_base::*;\n\
         pub use mode::*;\n\
         \n\
         /// Number of coefficient-CDF quality buckets (C `TOKEN_CDF_Q_CTXS`).\n\
         pub const TOKEN_CDF_Q_CTXS: usize = 4;\n\
         \n\
         /// C `get_q_ctx`: map base_qindex to the coefficient-CDF bucket.\n\
         #[inline]\n\
         pub fn coef_q_ctx(base_qindex: u8) -> usize {\n\
             match base_qindex {\n\
                 0..=20 => 0,\n\
                 21..=60 => 1,\n\
                 61..=120 => 2,\n\
                 _ => 3,\n\
             }\n\
         }\n",
    );
    let mut parts: std::collections::BTreeMap<&str, String> = FILES
        .iter()
        .map(|f| {
            (
                *f,
                "//! Generated by `gen_default_cdfs`; see `super`.\n\n\
                 use super::AomCdfProb;\n\n"
                    .to_string(),
            )
        })
        .collect();

    for t in TABLES {
        let part = parts.get_mut(file_for(t)).expect("known file");
        if t.q_dependent {
            let mut all = Vec::new();
            let mut per_q_len = 0usize;
            for &q in &Q_REPS {
                fc_init(q);
                let data = fc_table(t.table);
                per_q_len = data.len();
                all.extend_from_slice(&data);
            }
            let mut dims = vec![4usize];
            dims.extend_from_slice(t.dims);
            assert_eq!(
                per_q_len,
                t.dims.iter().product::<usize>(),
                "{}",
                t.rust_name
            );
            emit_table(part, t.rust_name, &all, &dims);
        } else {
            fc_init(60);
            let data = fc_table(t.table);
            emit_table(part, t.rust_name, &data, t.dims);
        }
    }

    let write = |path: String, text: &str| {
        std::fs::write(&path, text.trim_end().to_string() + "\n")
            .unwrap_or_else(|e| panic!("write {path}: {e}"));
        eprintln!("wrote {path}");
    };
    write(format!("{dir}/default_cdfs.rs"), &out);
    std::fs::create_dir_all(format!("{dir}/default_cdfs")).expect("mkdir default_cdfs");
    for (name, text) in &parts {
        write(format!("{dir}/default_cdfs/{name}.rs"), text);
    }
}
