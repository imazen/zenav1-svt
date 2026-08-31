//! Wholesale port of SVT-AV1's mode-decision layer —
//! `Source/Lib/Codec/mode_decision.c` and
//! `Source/Lib/Codec/product_coding_loop.c`.
//!
//! # Why this module exists
//!
//! Everything the inter path needs *below* mode decision is already
//! ported: open-loop ME ([`crate::inter_me`]), the reference-MV stack
//! ([`crate::inter_mvp`]), MV entropy and MV rate
//! ([`crate::inter_mv_code`]), and the MC / warp / OBMC / SAD / variance
//! DSP. Nothing in the encoder *reads* any of it, because the consumer
//! layer — candidate injection, the MD-level searches, the PD0 arms — had
//! no counterpart at all. This module is that layer.
//!
//! # Layout
//!
//! | module | C region |
//! |---|---|
//! | [`predicates`] | the pure gates and tables of `mode_decision.c` |
//! | [`pme`] | the MD motion-search cost model + the PME SAD kernel |
//! | [`nics`] | the per-stage candidate counts (`svt_aom_set_nics`) |
//! | [`drl`] | DRL selection for NEWMV candidates |
//! | [`coding_loop`] | the per-block helpers of `product_coding_loop.c` |
//! | [`inject`] | inter-candidate injection (PD1 and PD0) |
//!
//! # Reachability
//!
//! Nothing here is wired into `pipeline.rs` yet — the public entry point
//! still refuses inter frames. Per `docs/WORKING-ON-THIS.md` §7 a faithful
//! translation with no caller stays translated and states its
//! reachability rather than carrying `#[allow(dead_code)]`.

pub mod coding_loop;
pub mod drl;
pub mod inject;
pub mod nics;
pub mod pme;
pub mod predicates;
