//! Reference bindings for `Codec/rc_vbr_cbr.c`'s VBR/CBR state machine
//! (lane `wx-rc`).
//!
//! **Evidence tier 1 with one qualification, stated first.** Five of the
//! file's functions survive the Release build as local (`t`) symbols in
//! `cbuild-static/Source/Lib/Codec/CMakeFiles/CODEC.dir/rc_vbr_cbr.c.o`:
//! `av1_rc_regulate_q`, `av1_rc_update_rate_correction_factors`,
//! `calc_active_worst_quality_no_stats_cbr`, `get_regulated_q_overshoot`,
//! `get_regulated_q_undershoot` and `clamp_qindex`. `build.rs` promotes five
//! of them (see the exclusion below) with
//! `llvm-objcopy --globalize-symbol` on a private copy of that object and
//! links it ahead of `libSvtAv1Enc.a`, exactly as it already does for
//! `pd_process.c` — so the differential drives the REAL compiled C, not a
//! transcription. `svt_av1_resize_reset_rc` is plainly exported and needs no
//! promotion.
//!
//! TWO QUALIFICATIONS, both of which a reader has to know before trusting a
//! green run here:
//!
//! 1. `nm` shows the file's other ~40 statics were inlined away and have no
//!    symbol at any linkage, so no mechanism reaches them. They remain
//!    evidence tier 4 in the port and say so.
//! 2. `calc_active_worst_quality_no_stats_cbr` has a symbol but **LLVM
//!    specialized its ABI** — the compiled prologue takes two arguments and
//!    `x0` is not a `PictureParentControlSet*`. Binding it as declared
//!    returned 0 for every input. It is therefore NOT bound here; it is
//!    driven indirectly through the exported `svt_av1_resize_reset_rc`, which
//!    calls it. See `link_globalized_rc_vbr_statics` in `build.rs` for the
//!    disassembly.
//!
//! **The state travels as one `#[repr(C)]` struct.** These functions read a
//! `PictureParentControlSet`, a `SequenceControlSet`, an `EncodeContext` and
//! an `Av1Common` — four structs totalling tens of kilobytes, of which they
//! touch about forty scalars. [`RefRcVbrState`] is those forty scalars flat;
//! `shims/rc_vbr_cbr_shims.c` unpacks it into calloc'd control sets, calls the
//! real function, and packs the mutated fields back. Passing them as forty
//! positional arguments instead would make a transposition invisible.

/// Whether `build.rs` could promote `rc_vbr_cbr.c`'s five surviving statics on
/// this host.
///
/// The SKIP DECISION BELONGS TO THE CALLER, never to a test body (the
/// project's no-silent-skip rule): set `SVT_CREF_REQUIRE_RC_VBR_STATICS=1` and
/// [`rc_vbr_statics_oracle_is_available`] turns an unavailable oracle into a
/// loud failure instead of a quietly narrower suite.
pub const RC_VBR_STATICS_AVAILABLE: bool = cfg!(rc_vbr_statics);

/// Fail loudly when the caller demanded the tier-1 oracle and the host cannot
/// provide it. Call this from a test so an unavailable oracle is visible.
///
/// # Panics
/// When `SVT_CREF_REQUIRE_RC_VBR_STATICS` is set to a non-empty, non-`0` value
/// and the promotion did not happen.
pub fn rc_vbr_statics_oracle_is_available() -> bool {
    if !RC_VBR_STATICS_AVAILABLE {
        let required = std::env::var("SVT_CREF_REQUIRE_RC_VBR_STATICS")
            .map(|v| !v.is_empty() && v != "0")
            .unwrap_or(false);
        assert!(
            !required,
            "SVT_CREF_REQUIRE_RC_VBR_STATICS is set but build.rs could not promote \
             rc_vbr_cbr.c's statics — see its cargo:warning for which of the object file \
             or llvm-objcopy is missing."
        );
    }
    RC_VBR_STATICS_AVAILABLE
}

/// The `RATE_CONTROL` / `RateControlCfg` / `PictureParentControlSet` /
/// `CyclicRefresh` / `SequenceControlSet` scalars `rc_vbr_cbr.c`'s state
/// machine reads or writes.
///
/// Layout must match `RefRcVbrState` in `shims/rc_vbr_cbr_shims.c`
/// field-for-field; both are written in the same order with the same widths.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RefRcVbrState {
    // --- RATE_CONTROL ---
    pub avg_frame_bandwidth: i32,
    pub prev_avg_frame_bandwidth: i32,
    pub max_frame_bandwidth: i32,
    pub optimal_buffer_level: i64,
    pub maximum_buffer_size: i64,
    pub buffer_level: i64,
    pub bits_off_target: i64,
    pub rate_correction_factors: [f64; 7],
    pub avg_frame_qindex: [i32; 2],
    pub worst_quality: i32,
    pub best_quality: i32,
    pub q_1_frame: i32,
    pub q_2_frame: i32,
    pub rc_1_frame: i32,
    pub rc_2_frame: i32,
    pub frames_since_key: i32,
    pub percent_refresh_adjustment: i32,
    pub rate_ratio_qdelta_adjustment: f64,
    pub cur_avg_base_me_dist: u32,
    pub prev_avg_base_me_dist: u32,
    pub frame_updated: i32,
    // --- RateControlCfg ---
    /// `enum aom_rc_mode`: 0 VBR, 1 CBR, 2 Q.
    pub rc_mode: i32,
    pub under_shoot_pct: i32,
    pub over_shoot_pct: i32,
    // --- PictureParentControlSet ---
    /// `FrameType`: 0 KEY, 1 INTER, 2 INTRA_ONLY, 3 SWITCH.
    pub frame_type: i32,
    /// `SvtAv1FrameUpdateType`.
    pub update_type: i32,
    pub is_overlay: i32,
    pub sc_class1: i32,
    pub temporal_layer_index: i32,
    pub hierarchical_levels: i32,
    pub frame_width: i32,
    pub frame_height: i32,
    pub this_frame_target: i32,
    pub projected_frame_size: i32,
    pub base_q_idx: i32,
    pub b64_total_count: i32,
    // --- CyclicRefresh ---
    pub apply_cyclic_refresh: i32,
    pub qindex_delta: [i32; 3],
    pub actual_num_seg1_sbs: i32,
    pub actual_num_seg2_sbs: i32,
    // --- SequenceControlSet ---
    pub encoder_bit_depth: i32,
}

#[cfg(rc_vbr_statics)]
unsafe extern "C" {
    fn ref_rc_regulate_q(
        st: *mut RefRcVbrState,
        active_best_quality: i32,
        active_worst_quality: i32,
        width: i32,
        height: i32,
    ) -> i32;
    fn ref_rc_clamp_qindex(min_qp_allowed: i32, max_qp_allowed: i32, qindex: i32) -> i32;
    fn ref_rc_update_rate_correction_factors(st: *mut RefRcVbrState, width: i32, height: i32);
    fn ref_get_regulated_q_overshoot(
        st: *mut RefRcVbrState,
        q_low: i32,
        q_high: i32,
        top_index: i32,
        bottom_index: i32,
    ) -> i32;
    fn ref_get_regulated_q_undershoot(
        st: *mut RefRcVbrState,
        q_high: i32,
        top_index: i32,
        bottom_index: i32,
    ) -> i32;
}

unsafe extern "C" {
    fn ref_rc_resize_reset_rc(
        st: *mut RefRcVbrState,
        resize_width: i32,
        resize_height: i32,
        prev_width: i32,
        prev_height: i32,
    );
}

/// Reference `av1_rc_regulate_q` (rc_vbr_cbr.c:307).
///
/// `state` is updated in place because the CBR arm's `adjust_q_cbr` reads —
/// and `get_rate_correction_factor` locks — live RC state; nothing on this
/// path writes it, but the round trip is kept uniform across all five entry
/// points so a future C change cannot silently drop an output.
///
/// `None` when the tier-1 promotion is unavailable on this host.
#[must_use]
#[allow(unused_variables)]
pub fn regulate_q(
    state: &mut RefRcVbrState,
    active_best_quality: i32,
    active_worst_quality: i32,
    width: i32,
    height: i32,
) -> Option<i32> {
    #[cfg(rc_vbr_statics)]
    {
        Some(unsafe {
            ref_rc_regulate_q(
                state,
                active_best_quality,
                active_worst_quality,
                width,
                height,
            )
        })
    }
    #[cfg(not(rc_vbr_statics))]
    {
        None
    }
}

/// Reference `clamp_qindex` (rc_vbr_cbr.c:21), reached through the promotion.
///
/// The arguments are the CLI-domain `min_qp_allowed` / `max_qp_allowed`; C
/// maps both through the exported `quantizer_to_qindex` table itself, so this
/// pins the table lookup as well as the clip.
#[must_use]
#[allow(unused_variables)]
pub fn clamp_qindex(min_qp_allowed: i32, max_qp_allowed: i32, qindex: i32) -> Option<i32> {
    #[cfg(rc_vbr_statics)]
    {
        Some(unsafe { ref_rc_clamp_qindex(min_qp_allowed, max_qp_allowed, qindex) })
    }
    #[cfg(not(rc_vbr_statics))]
    {
        None
    }
}

/// Reference `av1_rc_update_rate_correction_factors` (rc_vbr_cbr.c:1354).
///
/// Returns whether the call happened; `state` carries every output
/// (`rate_correction_factors`, `q_1_frame`/`q_2_frame`, `rc_1_frame`/
/// `rc_2_frame` and the two cyclic-refresh adjustments).
#[allow(unused_variables)]
pub fn update_rate_correction_factors(state: &mut RefRcVbrState, width: i32, height: i32) -> bool {
    #[cfg(rc_vbr_statics)]
    {
        unsafe { ref_rc_update_rate_correction_factors(state, width, height) };
        true
    }
    #[cfg(not(rc_vbr_statics))]
    {
        false
    }
}

/// Reference `get_regulated_q_overshoot` (rc_vbr_cbr.c:1719).
#[must_use]
#[allow(unused_variables)]
pub fn get_regulated_q_overshoot(
    state: &mut RefRcVbrState,
    q_low: i32,
    q_high: i32,
    top_index: i32,
    bottom_index: i32,
) -> Option<i32> {
    #[cfg(rc_vbr_statics)]
    {
        Some(unsafe {
            ref_get_regulated_q_overshoot(state, q_low, q_high, top_index, bottom_index)
        })
    }
    #[cfg(not(rc_vbr_statics))]
    {
        None
    }
}

/// Reference `get_regulated_q_undershoot` (rc_vbr_cbr.c:1737).
#[must_use]
#[allow(unused_variables)]
pub fn get_regulated_q_undershoot(
    state: &mut RefRcVbrState,
    q_high: i32,
    top_index: i32,
    bottom_index: i32,
) -> Option<i32> {
    #[cfg(rc_vbr_statics)]
    {
        Some(unsafe { ref_get_regulated_q_undershoot(state, q_high, top_index, bottom_index) })
    }
    #[cfg(not(rc_vbr_statics))]
    {
        None
    }
}

/// Reference `svt_av1_resize_reset_rc` (rc_vbr_cbr.c:324) — EXPORTED, so this
/// one is tier 1 on every host with no promotion needed.
///
/// `state.this_frame_target` is an output.
pub fn resize_reset_rc(
    state: &mut RefRcVbrState,
    resize_width: i32,
    resize_height: i32,
    prev_width: i32,
    prev_height: i32,
) {
    unsafe { ref_rc_resize_reset_rc(state, resize_width, resize_height, prev_width, prev_height) };
}

// ---------------------------------------------------------------------------
// `svt_av1_rc_calc_qindex_rate_control` — the qindex DECISION chain
// ---------------------------------------------------------------------------
//
// EXPORTED, so tier 1 on every host with no promotion. It is the ONLY route
// to `rc_pick_q_and_bounds{,_no_stats_cbr}`,
// `calc_active_best_quality_no_stats_cbr`, `get_active_best_quality`,
// `adjust_active_best_and_worst_quality_org`, `av1_frame_type_qdelta_org`,
// `get_q`, `find_min_ref_base_q_idx` and `cyclic_refresh_init` — all of which
// C inlined into it, leaving no symbol of their own at any linkage.

/// The maximum DPB slots per list the qpick harness models. C's
/// `REF_LIST_MAX_DEPTH` is larger; four is what the sweeps use and the shim's
/// arrays are sized to match.
pub const QPICK_MAX_REFS: usize = 4;
/// The maximum 64x64 blocks the qpick harness models (the ME-distortion
/// arrays the VBR reference-floor arm sums over).
pub const QPICK_MAX_B64: usize = 64;

/// One DPB slot, as both the `EbReferenceObject` and the PCS mirrors of it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RefRcQpickRef {
    /// 0 leaves the slot empty (a NULL `ref_pic_ptr_array` entry).
    pub present: i32,
    pub tmp_layer_idx: i32,
    /// `ref_obj->slice_type`: 0 B_SLICE, 1 I_SLICE.
    pub slice_type: i32,
    /// `pcs->ref_slice_type[list][i]` — a SEPARATE mirror in C, kept separate
    /// here because two functions in the same file read different ones.
    pub pcs_slice_type: i32,
    pub ref_poc: u64,
    /// `pcs->ref_base_q_idx[list][i]`.
    pub base_q_idx: i32,
    /// `pcs->ref_pic_r0[list][i]`.
    pub pcs_r0: f64,
    /// `ref_obj->r0`.
    pub obj_r0: f64,
}

/// Everything `svt_av1_rc_calc_qindex_rate_control` reads or writes, flat.
///
/// Layout must match `RefRcQpickState` in `shims/rc_vbr_cbr_shims.c`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RefRcQpickState {
    /// The RATE_CONTROL / cfg / frame / cyclic-refresh / seq scalars shared
    /// with the state-machine oracle above.
    pub base: RefRcVbrState,

    // --- extra RATE_CONTROL ---
    pub active_worst_quality: i32,
    pub active_best_quality: [i32; 7],
    pub last_boosted_qindex: i32,
    pub kf_boost: i32,
    pub gfu_boost: i32,
    /// In and out.
    pub arf_q: i32,
    pub frames_to_key: i32,
    pub this_key_frame_forced: i32,
    pub avg_frame_low_motion: i32,

    // --- extra PPCS / PCS ---
    pub picture_number: u64,
    pub frame_offset: u64,
    /// `pcs->slice_type`: 0 B_SLICE, 1 I_SLICE.
    pub slice_type: i32,
    pub layer_depth: i32,
    pub is_ref: i32,
    pub transition_present: i32,
    pub r0: f64,

    // --- SequenceControlSet ---
    pub intra_period_length: i32,
    pub gop_constraint_rc: i32,
    pub is_short_clip: i32,
    pub super_block_size: i32,
    pub sb_total_count: i32,
    pub passes: i32,
    pub qp_scale_compress_strength: i32,
    pub input_resolution: i32,
    pub min_qp_allowed: i32,
    pub max_qp_allowed: i32,
    /// `scs->static_config.hierarchical_levels`. **Not** the PPCS field of the
    /// same name in [`RefRcVbrState`]: `cyclic_refresh_init` reads this one and
    /// `adjust_q_cbr` reads that one, in the same file.
    pub seq_hierarchical_levels: i32,

    // --- TWO_PASS ---
    pub extend_minq: i32,
    pub extend_maxq: i32,
    pub extend_minq_fast: i32,
    pub kf_zeromotion_pct: i32,

    // --- DPB ---
    pub l0_count_try: i32,
    pub l1_count_try: i32,
    pub l0: [RefRcQpickRef; QPICK_MAX_REFS],
    pub l1: [RefRcQpickRef; QPICK_MAX_REFS],

    // --- ME distortion ---
    pub b64_total_count: i32,
    pub me_cur_64x64: [u32; QPICK_MAX_B64],
    pub me_cur_8x8: [u32; QPICK_MAX_B64],
    pub me_ref_l0_64x64: [u32; QPICK_MAX_B64],
    pub me_mv_x: [i32; QPICK_MAX_B64],
    pub me_mv_y: [i32; QPICK_MAX_B64],
    pub norm_me_dist: u64,

    // --- cyclic refresh, in and out ---
    /// `enc_ctx->cr_sb_end` — the cursor that walks the refresh band across
    /// pictures. In AND out.
    pub cr_sb_end_ctx: u32,
    pub cr_apply: i32,
    pub cr_percent_refresh: i32,
    pub cr_sb_start: u32,
    pub cr_sb_end: u32,
    pub cr_max_qdelta_perc: i32,
    pub cr_rate_boost_fac: i32,
    pub cr_rate_ratio_qdelta: f64,
    pub cr_rate_ratio_qdelta_seg2: f64,
    pub cr_qindex_delta: [i32; 3],
    pub cr_actual_num_seg1_sbs: i32,
    pub cr_actual_num_seg2_sbs: i32,

    // --- outputs ---
    pub out_base_q_idx: i32,
    pub out_top_index: i32,
    pub out_bottom_index: i32,
}

impl Default for RefRcQpickState {
    fn default() -> Self {
        Self {
            base: RefRcVbrState::default(),
            active_worst_quality: 0,
            active_best_quality: [0; 7],
            last_boosted_qindex: 0,
            kf_boost: 0,
            gfu_boost: 0,
            arf_q: 0,
            frames_to_key: 0,
            this_key_frame_forced: 0,
            avg_frame_low_motion: 0,
            picture_number: 0,
            frame_offset: 0,
            slice_type: 0,
            layer_depth: 0,
            is_ref: 0,
            transition_present: 0,
            r0: 0.0,
            intra_period_length: -1,
            gop_constraint_rc: 0,
            is_short_clip: 0,
            super_block_size: 64,
            sb_total_count: 0,
            passes: 1,
            qp_scale_compress_strength: 0,
            input_resolution: 0,
            min_qp_allowed: 1,
            max_qp_allowed: 63,
            seq_hierarchical_levels: 0,
            extend_minq: 0,
            extend_maxq: 0,
            extend_minq_fast: 0,
            kf_zeromotion_pct: 0,
            l0_count_try: 0,
            l1_count_try: 0,
            l0: [RefRcQpickRef::default(); QPICK_MAX_REFS],
            l1: [RefRcQpickRef::default(); QPICK_MAX_REFS],
            b64_total_count: 0,
            me_cur_64x64: [0; QPICK_MAX_B64],
            me_cur_8x8: [0; QPICK_MAX_B64],
            me_ref_l0_64x64: [0; QPICK_MAX_B64],
            me_mv_x: [0; QPICK_MAX_B64],
            me_mv_y: [0; QPICK_MAX_B64],
            norm_me_dist: 0,
            cr_sb_end_ctx: 0,
            cr_apply: 0,
            cr_percent_refresh: 0,
            cr_sb_start: 0,
            cr_sb_end: 0,
            cr_max_qdelta_perc: 0,
            cr_rate_boost_fac: 0,
            cr_rate_ratio_qdelta: 0.0,
            cr_rate_ratio_qdelta_seg2: 0.0,
            cr_qindex_delta: [0; 3],
            cr_actual_num_seg1_sbs: 0,
            cr_actual_num_seg2_sbs: 0,
            out_base_q_idx: 0,
            out_top_index: 0,
            out_bottom_index: 0,
        }
    }
}

unsafe extern "C" {
    fn ref_rc_calc_qindex_rate_control(st: *mut RefRcQpickState) -> i32;
}

/// Reference `svt_av1_rc_calc_qindex_rate_control` (rc_vbr_cbr.c:1281).
///
/// Returns the new `base_q_idx`; `state` also carries `out_top_index`,
/// `out_bottom_index`, the mutated `arf_q` / `active_worst_quality` /
/// `cr_sb_end_ctx`, and the whole post-call `CyclicRefresh`.
pub fn calc_qindex_rate_control(state: &mut RefRcQpickState) -> i32 {
    unsafe { ref_rc_calc_qindex_rate_control(state) }
}
