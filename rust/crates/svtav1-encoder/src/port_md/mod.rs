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
//! | [`nic_prune`] | the per-stage candidate STAGING (sorts + NIC prunes) |
//! | [`lpd1`] | the light-PD1 gates + the chroma-complexity detectors |
//! | [`lpd1_loop`] | the light-PD1 MDS0 cost + candidate walk |
//! | [`nsq_skip`] | the NSQ-shape skip gates, inter modes and AB shapes live |
//! | [`drl`] | DRL selection for NEWMV candidates |
//! | [`coding_loop`] | the per-block helpers of `product_coding_loop.c` |
//! | [`inject`] | inter-candidate injection (PD1 and PD0) |
//! | [`ssim_hbd`] | the high-bit-depth arm of the tune-SSIM distortion |
//! | [`md_search`] | the MD-level full-pel / sub-pel / PME searches |
//! | [`motion_mode`] | motion-mode refinement, inter-intra, PD0 staging |
//! | [`ref_frame_rate`] | the reference-signalling rate + its contexts |
//! | [`mv_refine`] | the WM / OBMC motion-mode MV refinements |
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
pub mod lpd1;
pub mod lpd1_loop;
pub mod md_search;
pub mod motion_mode;
pub mod mv_refine;
pub mod nic_prune;
pub mod nics;
pub mod nsq_skip;
pub mod pme;
pub mod predicates;
pub mod ref_frame_rate;
pub mod ssim_hbd;
pub mod tx_gates;
