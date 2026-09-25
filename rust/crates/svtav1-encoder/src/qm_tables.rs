//! AV1 quantization-matrix weight tables, transcribed verbatim from
//! SVT-AV1 `q_matrices.h` (`wt_matrix_ref` / `iwt_matrix_ref`) by
//! `xtask/transcribe_qm.py`. Do not hand-edit. Validated end-to-end by
//! `tests/c_parity_qm.rs`, which feeds these slices as qm/iqm pointers to
//! the exported C quantize kernels and compares against the Rust port.
//!
//! Layout matches C: per level, per {luma, chroma}, the concatenation of
//! one flattened matrix per SELF-ADJUSTED tx size in TX_SIZES_ALL order
//! (see `qm::qm_offset`). Level 15 has no matrices (identity).

pub const QM_TOTAL_SIZE: usize = 3344;

mod iwt_matrix;
mod wt_matrix;
pub use iwt_matrix::IWT_MATRIX_REF;
pub use wt_matrix::WT_MATRIX_REF;
