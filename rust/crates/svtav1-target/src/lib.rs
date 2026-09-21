#![forbid(unsafe_code)]

//! Target-quality encode loop for zenav1-svt stills: name a metric and a
//! target value; the crate brackets qp, encodes real trials through
//! `EncodePipeline`, and returns the best trial actually encoded —
//! never an interpolated result.
//!
//! Registered wave: `benchmarks/zensim_hdr_target_wave_2026-08-27.md` —
//! this crate is the codec-owned loop home (per-codec loop ownership),
//! SEPARATE from the byte-gated C-parity crates, which must never grow a
//! zensim dependency.
//!
//! Layers:
//! - [`search`]: the bracketed qp search (+ secant two-shot policy).
//! - [`metric`]: [`MetricKind`], direction/`sign`, per-metric anchor
//!   tables and slope bounds — the multi-metric contract.
//! - [`trial`]: [`TargetSpec`], [`MetricJudge`], and the bd8/bd10
//!   `encode_to_target_*` drivers with the `configure` tune hook.
//! - [`seed`]: the original S1 zensim anchors + `TargetOptions::seeded`.

pub mod metric;
pub mod search;
pub mod seed;
pub mod trial;

pub use metric::{MetricKind, MetricScore, anchor_qp_start, score_of};
pub use search::{StepPolicy, TargetOptions, TargetSearchResult, search_target_qp};
pub use trial::{
    MetricJudge, TargetEncodeResult, TargetError, TargetSpec, TrialOutput, TrialRecon,
    encode_to_target_8bit, encode_to_target_10bit,
};
