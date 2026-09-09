//! Aggregated integration-test entry point.
//!
//! Every file in `tests/` used to be its own crate AND its own linked
//! executable -- 182 of them workspace-wide, 76-97% DWARF each, 5.62 GB per
//! clean build (measured 2026-09-09). Declaring them as modules of one target
//! links once instead of once per file. The files are NOT moved, so every
//! `include_bytes!("data/...")` path still resolves.
//!
//! DELIBERATELY NOT MERGED, and each still its own `[[test]]` target:
//!   * files calling `archmage::testing::for_each_token_permutation` or
//!     `lock_token_testing` -- token disabling is PROCESS-WIDE, and this repo
//!     also runs plain `cargo test` (threaded, one process) from CI and from
//!     tools/regression_spotcheck.sh, where merging them would let one test
//!     flip the dispatch tier underneath another.
//!   * files named directly by a gate: `--test tier_invariance` (CI),
//!     `--test odd_frame_recon` and `--test still_policy` (spotcheck).

mod configuration_support;
mod e2e_correctness;
mod golden_parity;
mod hdr_fork_e2e;
mod issue11_repro;
mod issue13_repro;
mod issue18_repro;
mod issue9_repro;
mod real_encode;
mod thread_determinism;
