//! Open-loop motion estimation — a wholesale port of SVT-AV1's
//! `Source/Lib/Codec/motion_estimation.c` (2,964 lines).
//!
//! # Why this module exists
//!
//! `crate::motion_est` is a **homegrown** full-pel + bilinear-subpel searcher.
//! It is not SVT's algorithm, and "the ME is homegrown" is one of the named
//! reasons `pipeline.rs` refuses inter frames. This module is the real thing:
//! the pyramid (pre-HME + HME levels 0/1/2), the search-area derivation, the
//! integer full-pel search over the eight-point and single-point SAD kernels,
//! the reference-pruning ladders, the ME candidate arrays, the global-motion
//! detector and the per-block distortion summary — transcribed function by
//! function from the C.
//!
//! Nothing in the encoder calls it yet. Switching the call sites is a separate
//! chunk (see the module list at the bottom of this doc).
//!
//! # Layout
//!
//! | module | C region |
//! |---|---|
//! | [`sad`] | motion_estimation.c:43-405 + `compute_sad_c.c` loop kernels |
//! | [`context`] | `me_context.h` + the `pcs` fields ME reads |
//! | [`hme`] | motion_estimation.c:787-1152, 1458-2333 (the pyramid) |
//! | [`integer`] | motion_estimation.c:408-786, 1163-1456 (full-pel) |
//! | [`candidates`] | motion_estimation.c:2335-2786 |
//! | [`b64`] | motion_estimation.c:2788-2964 (the entry point) |
//! | [`tables`] | `tab8x8`, `z_to_raster`, the two ME index maps |
//!
//! # Coverage against the C surface
//!
//! All 40 functions defined in `motion_estimation.c` have a counterpart here.
//! Two C constructs deliberately have none:
//!
//! * `get_me_reference`'s `SVT_WARN` on a pyramid-resolution mismatch — a log
//!   line with no effect on the search. Its `*dist` output IS ported, as
//!   [`hme::get_me_reference_dist`].
//! * The `tf_*` half of `MeContext`. `motion_estimation.c` reads five of those
//!   fields and this port carries exactly those five; the rest belong to
//!   `temporal_filtering.c`.
//!
//! `av1me.c` is NOT ported here: its IntraBC half already lives in
//! [`crate::intrabc`] (`svt_av1_full_pixel_search`, `svt_av1_diamond_search_sad_c`,
//! `exhaustive_mesh_search`, `svt_av1_refining_search_sad`, `full_pixel_diamond`,
//! `intrabc_full_pixel_exhaustive`, `svt_av1_set_mv_search_range`,
//! `svt_av1_init3smotion_compensation`, `svt_av1_get_mvpred_var`,
//! `svt_aom_mv_err_cost{,_light}`, `mvsad_err_cost{,_light}`), and its OBMC half
//! is unported — see the crate-level chunk notes.
//!
//! # Evidence
//!
//! Six of the C functions ported here are exported symbols and are gated at
//! **tier 1** (`docs/WORKING-ON-THIS.md` §4) by
//! `tests/c_parity_inter_me.rs`, which drives the real
//! `libSvtAv1Enc.a`:
//! `svt_aom_compute8x4_sad_kernel_c`, `svt_ext_sad_calculation_8x8_16x16_c`,
//! `svt_ext_sad_calculation_32x32_64x64_c`,
//! `svt_ext_all_sad_calculation_8x8_16x16_c`,
//! `svt_ext_eight_sad_calculation_32x32_64x64_c`,
//! `svt_sad_loop_kernel_c`, `svt_nxm_sad_kernel_helper_c`,
//! `svt_aom_get_scaled_picture_distance`, `hme_level_2` and `check_00_center`.
//!
//! Everything else in `motion_estimation.c` is `static` and has **no** exported
//! symbol, so it can only reach **tier 4** — hand-derived vectors traced
//! against the C source. Those tests say so in their own doc comments. A tier-4
//! test proves the port is self-consistent and matches a hand-trace; it does
//! NOT prove parity with the C binary, and must never be described as if it
//! did.

pub mod b64;
pub mod candidates;
pub mod context;
pub mod hme;
pub mod integer;
pub mod sad;
pub mod tables;

pub use b64::{init_me_hme_data, me_static_b64_bypass, motion_estimation_b64};
pub use context::{
    MeB64Output, MeCandidate, MeContext, MeDsRef, MePicParams, MeRefs, MeSrcBufs, MeType, Plane, SearchArea,
    SearchAreaMinMax,
};
