#![forbid(unsafe_code)]

//! zensim HDR target loop for zenav1-svt stills.
//!
//! Registered wave: `benchmarks/zensim_hdr_target_wave_2026-08-27.md` —
//! this crate is the codec-owned loop home (per-codec loop ownership),
//! SEPARATE from the byte-gated C-parity crates, which must never grow a
//! zensim dependency.
//!
//! Chunk 1 (this): the pure bracketed-qp search + options, unit-tested.
//! Chunk 2: the trial cell — `EncodePipeline` at CQP with
//! `with_recon_output(true)` (the encoder's own normative reconstruction;
//! no decode-back dependency) → PU-domain judge.

pub mod search;
pub mod seed;
pub mod trial;

pub use search::{TargetOptions, TargetSearchResult, search_target_qp};
pub use trial::{TargetError, TrialOutput, encode_to_target};
