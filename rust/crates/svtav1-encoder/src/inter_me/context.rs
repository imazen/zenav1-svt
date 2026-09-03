//! `MeContext` and the picture-side inputs/outputs the open-loop ME reads and
//! writes — a port of `Source/Lib/Codec/me_context.h` plus the handful of
//! `PictureParentControlSet` / `MeSbResults` fields `motion_estimation.c`
//! actually touches.
//!
//! **What is NOT here, and why.** `MeContext` in C is one struct carrying the
//! open-loop ME state, the temporal-filter (`ME_MCTF`) state and raw pointers
//! into the reference pictures. This port keeps the ME state, keeps only the
//! five TF fields `motion_estimation.c` itself reads
//! (`me_type`, `tf_me_exit_th`, `tf_use_pred_64x64_only_th`,
//! `tf_tot_horz_blks`, `tf_tot_vert_blks`), and moves the buffers out into
//! [`MeSrcBufs`] / [`MeRefs`] so the borrow checker can see that ME reads the
//! references and writes only the context. The remaining `tf_*` fields belong
//! to `temporal_filtering.c`, which is NOT in this chunk's scope.

use svtav1_types::motion::Mv;

/// C `MAX_NUM_OF_REF_PIC_LIST` (definitions.h:2048).
pub const MAX_NUM_OF_REF_PIC_LIST: usize = 2;
/// C `MAX_REF_IDX` (definitions.h:2049).
pub const MAX_REF_IDX: usize = 4;
/// C `REF_LIST_MAX_DEPTH` (API/EbSvtAv1Enc.h:35).
pub const REF_LIST_MAX_DEPTH: usize = 4;
/// C `SQUARE_PU_COUNT` (me_sb_results.h:25).
pub const SQUARE_PU_COUNT: usize = 85;
/// C `MAX_SB64_PU_COUNT_NO_8X8` (me_sb_results.h:26).
pub const MAX_SB64_PU_COUNT_NO_8X8: usize = 21;
/// C `MAX_SB64_PU_COUNT_WO_16X16` (me_sb_results.h:27).
pub const MAX_SB64_PU_COUNT_WO_16X16: usize = 5;
/// C `SEARCH_REGION_COUNT` (me_context.h:281).
pub const SEARCH_REGION_COUNT: usize = 2;
/// C `EB_HME_SEARCH_AREA_COLUMN_MAX_COUNT` (definitions.h:50).
pub const HME_SA_COL_MAX: usize = 2;
/// C `EB_HME_SEARCH_AREA_ROW_MAX_COUNT` (definitions.h:51).
pub const HME_SA_ROW_MAX: usize = 2;
/// C `BLOCK_SIZE_64` (definitions.h:2033).
pub const BLOCK_SIZE_64: i32 = 64;
/// C `ME_FILTER_TAP` (definitions.h:1819).
pub const ME_FILTER_TAP: i32 = 4;
/// C `SUB_SAD_SEARCH` (definitions.h:1820).
pub const SUB_SAD_SEARCH: u8 = 0;
/// C `FULL_SAD_SEARCH` (definitions.h:1821).
pub const FULL_SAD_SEARCH: u8 = 1;
/// C `MAX_SAD_VALUE` (motion_estimation.h:64).
pub const MAX_SAD_VALUE: u32 = 128 * 128 * 255;
/// C `COST_PRECISION` (lambda_rate_tables.h:19).
pub const COST_PRECISION: u32 = 8;
/// C `NUM_MV_COMPONENTS` (definitions.h:86).
pub const NUM_MV_COMPONENTS: usize = 2;
/// C `NUM_MV_HIST` (definitions.h:87).
pub const NUM_MV_HIST: usize = 2;
/// C `INPUT_SIZE_480p_RANGE` (definitions.h:1826).
pub const INPUT_SIZE_480P_RANGE: u8 = 2;
/// C `INPUT_SIZE_COUNT` (definitions.h:1831).
pub const INPUT_SIZE_COUNT: u32 = 7;
/// C `ME_TIER_ZERO_PU_64x64` (me_context.h:46).
pub const PU_64X64: usize = 0;
/// C `ME_TIER_ZERO_PU_32x32_0`.
pub const PU_32X32_0: usize = 1;
/// C `ME_TIER_ZERO_PU_16x16_0`.
pub const PU_16X16_0: usize = 5;
/// C `ME_TIER_ZERO_PU_8x8_0`.
pub const PU_8X8_0: usize = 21;
/// C `BI_PRED` — the `MeCandidate::direction` value for bi-prediction.
pub const BI_PRED: u8 = 2;

/// C `EbMeType` (me_context.h:36).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MeType {
    /// `ME_CLOSE_LOOP`.
    CloseLoop,
    /// `ME_MCTF` — the temporal filter's ME.
    Mctf,
    /// `ME_TPL`.
    Tpl,
    /// `ME_OPEN_LOOP` — the ordinary open-loop ME.
    #[default]
    OpenLoop,
    /// `ME_FIRST_PASS`.
    FirstPass,
    /// `ME_DG_DETECTOR`.
    DgDetector,
}

/// C `MeHmeRefPruneCtrls` (me_context.h:222).
#[derive(Clone, Copy, Debug, Default)]
pub struct MeHmeRefPruneCtrls {
    /// C `enable_me_hme_ref_pruning`.
    pub enable_me_hme_ref_pruning: bool,
    /// C `prune_ref_if_hme_sad_dev_bigger_than_th`.
    pub prune_ref_if_hme_sad_dev_bigger_than_th: u16,
    /// C `prune_ref_if_me_sad_dev_bigger_than_th`.
    pub prune_ref_if_me_sad_dev_bigger_than_th: u16,
    /// C `zz_sad_th`.
    pub zz_sad_th: u32,
    /// C `zz_sad_pct`.
    pub zz_sad_pct: u16,
    /// C `phme_sad_th`.
    pub phme_sad_th: u32,
    /// C `phme_sad_pct`.
    pub phme_sad_pct: u16,
}

/// C `MeSrCtrls` (me_context.h:233).
#[derive(Clone, Copy, Debug, Default)]
pub struct MeSrCtrls {
    /// C `enable_me_sr_adjustment`.
    pub enable_me_sr_adjustment: u8,
    /// C `reduce_me_sr_based_on_mv_length_th`.
    pub reduce_me_sr_based_on_mv_length_th: u16,
    /// C `stationary_hme_sad_abs_th`.
    pub stationary_hme_sad_abs_th: u16,
    /// C `stationary_me_sr_divisor`.
    pub stationary_me_sr_divisor: u16,
    /// C `reduce_me_sr_based_on_hme_sad_abs_th`.
    pub reduce_me_sr_based_on_hme_sad_abs_th: u16,
    /// C `me_sr_divisor_for_low_hme_sad`.
    pub me_sr_divisor_for_low_hme_sad: u16,
    /// C `distance_based_hme_resizing`.
    pub distance_based_hme_resizing: u8,
}

/// C `Me8x8VarCtrls` (me_context.h:252).
#[derive(Clone, Copy, Debug, Default)]
pub struct Me8x8VarCtrls {
    /// C `enabled`.
    pub enabled: u8,
    /// C `me_sr_div4_th`.
    pub me_sr_div4_th: u32,
    /// C `me_sr_div2_th`.
    pub me_sr_div2_th: u32,
    /// C `me_sr_mult2_th`.
    pub me_sr_mult2_th: u32,
}

/// C `MvBasedSearchAdj` (me_context.h:305).
#[derive(Clone, Copy, Debug, Default)]
pub struct MvBasedSearchAdj {
    /// C `enabled`.
    pub enabled: bool,
    /// C `nearest_ref_only`.
    pub nearest_ref_only: bool,
    /// C `mv_size_th`.
    pub mv_size_th: u16,
    /// C `sa_multiplier`.
    pub sa_multiplier: u16,
}

/// C `SearchArea` (me_context.h:283).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchArea {
    /// C `width`.
    pub width: u16,
    /// C `height`.
    pub height: u16,
}

/// C `SearchAreaMinMax` (me_context.h:288).
#[derive(Clone, Copy, Debug, Default)]
pub struct SearchAreaMinMax {
    /// C `sa_min`.
    pub sa_min: SearchArea,
    /// C `sa_max`.
    pub sa_max: SearchArea,
}

/// C `SearchInfo` (me_context.h:293).
#[derive(Clone, Copy, Debug, Default)]
pub struct SearchInfo {
    /// C `sa`.
    pub sa: SearchArea,
    /// C `best_mv`.
    pub best_mv: Mv,
    /// C `sad`.
    pub sad: u64,
    /// C `valid`.
    pub valid: u8,
}

/// C `PreHmeCtrls` (me_context.h:300).
#[derive(Clone, Copy, Debug, Default)]
pub struct PreHmeCtrls {
    /// C `enable`.
    pub enable: u8,
    /// C `prehme_sa_cfg`.
    pub prehme_sa_cfg: [SearchAreaMinMax; SEARCH_REGION_COUNT],
    /// C `skip_search_line`.
    pub skip_search_line: u8,
    /// C `l1_early_exit`.
    pub l1_early_exit: u8,
}

/// C `SearchResults` (me_context.h:307).
#[derive(Clone, Copy, Debug, Default)]
pub struct SearchResults {
    /// C `list_i`.
    pub list_i: u8,
    /// C `ref_i`.
    pub ref_i: u8,
    /// C `hme_sc_x`.
    pub hme_sc_x: i16,
    /// C `hme_sc_y`.
    pub hme_sc_y: i16,
    /// C `hme_sad`.
    pub hme_sad: u64,
    /// C `do_ref`.
    pub do_ref: u8,
}

/// C `MeCandidate` (me_sb_results.h:29). C declares all five fields as
/// bitfields of ONE `uint8_t` (`direction : 2`, `ref_idx_l* : 2`,
/// `ref*_list : 1` — 2+2+2+1+1 = 8 bits), so `sizeof(MeCandidate) == 1`, and
/// the port stores the same single byte.
///
/// **Why one byte and not five.** `me_candidate_array` is sized
/// `SQUARE_PU_COUNT * max_cand` per b64 and lives for the whole frame, so its
/// width is multiplied by the b64 count: five `pub u8` fields cost
/// `85 * 23 * 5 = 9,775` bytes per b64 against C's 1,955, and at
/// 2048x2048 (1,024 b64s) that is 10.01 MB of live heap against C's 2.00 MB.
/// MEASURED at the inter arm's peak with massif — `MeB64Output::reset` is
/// exactly the 10.01 MB entry of `benchmarks/mem_massif_2026-09-03.meta`.
/// Packing is byte-inert by construction: every write already masked to the
/// bitfield width, and the accessors below re-derive the same masked values.
///
/// The five accessors **mask on read** and [`MeCandidate::set`] masks on
/// write, so the truncation C performs — notably `ref0_list = 24` becoming
/// `0` — is reproduced rather than accidentally widened.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct MeCandidate {
    /// The five C bitfields in one byte. The BIT ORDER is the port's own —
    /// nothing reads this byte through an FFI boundary, only the accessors —
    /// so it does not have to match any particular C ABI's field packing.
    bits: u8,
}

impl MeCandidate {
    /// Store all five bitfields with C's truncation.
    pub fn set(
        &mut self,
        direction: u8,
        ref_idx_l0: u8,
        ref_idx_l1: u8,
        ref0_list: u8,
        ref1_list: u8,
    ) {
        self.bits = (direction & 0x3)
            | ((ref_idx_l0 & 0x3) << 2)
            | ((ref_idx_l1 & 0x3) << 4)
            | ((ref0_list & 0x1) << 6)
            | ((ref1_list & 0x1) << 7);
    }

    /// [`set`](Self::set) on a fresh value.
    #[must_use]
    pub fn new(
        direction: u8,
        ref_idx_l0: u8,
        ref_idx_l1: u8,
        ref0_list: u8,
        ref1_list: u8,
    ) -> Self {
        let mut c = Self::default();
        c.set(direction, ref_idx_l0, ref_idx_l1, ref0_list, ref1_list);
        c
    }

    /// C `direction : 2` — 0 = list-0 uni, 1 = list-1 uni, 2 = bi.
    #[inline]
    #[must_use]
    pub const fn direction(self) -> u8 {
        self.bits & 0x3
    }
    /// C `ref_idx_l0 : 2`.
    #[inline]
    #[must_use]
    pub const fn ref_idx_l0(self) -> u8 {
        (self.bits >> 2) & 0x3
    }
    /// C `ref_idx_l1 : 2`.
    #[inline]
    #[must_use]
    pub const fn ref_idx_l1(self) -> u8 {
        (self.bits >> 4) & 0x3
    }
    /// C `ref0_list : 1`.
    #[inline]
    #[must_use]
    pub const fn ref0_list(self) -> u8 {
        (self.bits >> 6) & 0x1
    }
    /// C `ref1_list : 1`.
    #[inline]
    #[must_use]
    pub const fn ref1_list(self) -> u8 {
        (self.bits >> 7) & 0x1
    }
}

impl core::fmt::Debug for MeCandidate {
    /// Print the five C fields, not the packed byte — every dump that reads
    /// this type reads it as C's five bitfields.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MeCandidate")
            .field("direction", &self.direction())
            .field("ref_idx_l0", &self.ref_idx_l0())
            .field("ref_idx_l1", &self.ref_idx_l1())
            .field("ref0_list", &self.ref0_list())
            .field("ref1_list", &self.ref1_list())
            .finish()
    }
}

/// A padded luma plane: C's `EbPictureBufferDesc` as ME uses it.
///
/// C's `y_buffer` points at pixel (0,0) *inside* a bordered allocation, and
/// every search-region index may be negative (into the left/top border). The
/// port therefore carries the whole allocation plus `org`, the index of (0,0).
#[derive(Clone, Copy, Debug)]
pub struct Plane<'a> {
    /// The whole padded allocation.
    pub data: &'a [u8],
    /// Index of pixel (0,0) — C's `y_buffer - buffer_y`.
    pub org: usize,
    /// C `y_stride`.
    pub stride: usize,
    /// C `width`.
    pub width: u16,
    /// C `height`.
    pub height: u16,
    /// C `border` (`origin_x`/`origin_y`).
    pub border: u16,
}

impl<'a> Plane<'a> {
    /// The tail of the allocation starting at `y_buffer[off]`, where `off` may
    /// be negative (into the border).
    #[inline]
    pub fn at(&self, off: i64) -> &'a [u8] {
        let idx = self.org as i64 + off;
        assert!(idx >= 0, "ME plane index {idx} is before the allocation");
        &self.data[idx as usize..]
    }

    /// The absolute allocation index of `y_buffer[off]`.
    #[inline]
    pub fn abs(&self, off: i64) -> i64 {
        self.org as i64 + off
    }
}

/// C `EbDownScaledBufDescPtrArray` as ME reads it: one reference picture at
/// full, quarter and sixteenth resolution plus its picture number.
#[derive(Clone, Copy, Debug)]
pub struct MeDsRef<'a> {
    /// C `picture_ptr` — full resolution.
    pub picture: Plane<'a>,
    /// C `quarter_picture_ptr`.
    pub quarter: Plane<'a>,
    /// C `sixteenth_picture_ptr`.
    pub sixteenth: Plane<'a>,
    /// C `picture_number`.
    pub picture_number: u64,
}

/// C `me_ctx->me_ds_ref_array` — the per-list/per-ref reference set.
#[derive(Clone, Copy, Debug, Default)]
pub struct MeRefs<'a> {
    /// `[list][ref]`; `None` for slots outside `num_of_ref_pic_to_search`.
    pub arr: [[Option<MeDsRef<'a>>; REF_LIST_MAX_DEPTH]; MAX_NUM_OF_REF_PIC_LIST],
}

impl<'a> MeRefs<'a> {
    /// The reference at `[list][ref_idx]`; panics when ME reaches a slot the
    /// caller did not populate (that is a caller bug, never a C behaviour).
    #[inline]
    pub fn get(&self, list: usize, ref_idx: usize) -> &MeDsRef<'a> {
        self.arr[list][ref_idx]
            .as_ref()
            .expect("ME reached an unpopulated reference slot")
    }
}

/// The three source-side b64 buffers C keeps in `MeContext`
/// (`b64_src_ptr`, `quarter_b64_buffer`, `sixteenth_b64_buffer`) together with
/// their strides.
#[derive(Clone, Copy, Debug)]
pub struct MeSrcBufs<'a> {
    /// C `b64_src_ptr`.
    pub b64: &'a [u8],
    /// C `b64_src_stride`.
    pub b64_stride: usize,
    /// C `quarter_b64_buffer`.
    pub quarter: &'a [u8],
    /// C `quarter_b64_buffer_stride`.
    pub quarter_stride: usize,
    /// C `sixteenth_b64_buffer`.
    pub sixteenth: &'a [u8],
    /// C `sixteenth_b64_buffer_stride`.
    pub sixteenth_stride: usize,
}

/// C `MeContext` (me_context.h:344), restricted to the open-loop ME state.
#[derive(Clone, Debug)]
pub struct MeContext {
    /// C `interpolated_full_stride`.
    pub interpolated_full_stride: [[usize; MAX_REF_IDX]; MAX_NUM_OF_REF_PIC_LIST],
    /// C `integer_buffer_ptr`, as an absolute index into the reference
    /// picture's allocation (see [`Plane::abs`]).
    pub integer_buffer_off: [[i64; MAX_REF_IDX]; MAX_NUM_OF_REF_PIC_LIST],
    /// C `me_distortion[SQUARE_PU_COUNT]`.
    pub me_distortion: [u32; SQUARE_PU_COUNT],
    /// C `p_sad32x32`.
    pub p_sad32x32: [u32; 4],
    /// C `p_sad16x16`.
    pub p_sad16x16: [u32; 16],
    /// C `p_sad8x8`.
    pub p_sad8x8: [u32; 64],
    /// C `p_sb_best_sad`.
    pub p_sb_best_sad: [[[u32; SQUARE_PU_COUNT]; MAX_REF_IDX]; MAX_NUM_OF_REF_PIC_LIST],
    /// C `p_sb_best_mv`.
    pub p_sb_best_mv: [[[u32; SQUARE_PU_COUNT]; MAX_REF_IDX]; MAX_NUM_OF_REF_PIC_LIST],
    /// C `p_eight_sad32x32`.
    pub p_eight_sad32x32: [[u32; 8]; 4],
    /// C `p_eight_sad16x16`.
    pub p_eight_sad16x16: [[u32; 8]; 16],
    /// C `hme_search_method`.
    pub hme_search_method: u8,
    /// C `me_search_method`.
    pub me_search_method: u8,
    /// C `enable_hme_flag`.
    pub enable_hme_flag: bool,
    /// C `enable_hme_level0_flag`.
    pub enable_hme_level0_flag: bool,
    /// C `enable_hme_level1_flag`.
    pub enable_hme_level1_flag: bool,
    /// C `enable_hme_level2_flag`.
    pub enable_hme_level2_flag: bool,
    /// C `me_hme_prune_ctrls`.
    pub me_hme_prune_ctrls: MeHmeRefPruneCtrls,
    /// C `me_sr_adjustment_ctrls`.
    pub me_sr_adjustment_ctrls: MeSrCtrls,
    /// C `me_8x8_var_ctrls`.
    pub me_8x8_var_ctrls: Me8x8VarCtrls,
    /// C `mv_based_sa_adj`.
    pub mv_based_sa_adj: MvBasedSearchAdj,
    /// C `best_list_idx`.
    pub best_list_idx: u8,
    /// C `best_ref_idx`.
    pub best_ref_idx: u8,
    /// C `me_sa`.
    pub me_sa: SearchAreaMinMax,
    /// C `num_hme_sa_w`.
    pub num_hme_sa_w: u16,
    /// C `num_hme_sa_h`.
    pub num_hme_sa_h: u16,
    /// C `hme_l0_sa`.
    pub hme_l0_sa: SearchAreaMinMax,
    /// C `hme_l1_sa`.
    pub hme_l1_sa: SearchArea,
    /// C `hme_l2_sa`.
    pub hme_l2_sa: SearchArea,
    /// C `search_results`.
    pub search_results: [[SearchResults; REF_LIST_MAX_DEPTH]; MAX_NUM_OF_REF_PIC_LIST],
    /// C `reduce_me_sr_divisor`.
    pub reduce_me_sr_divisor: [[u32; REF_LIST_MAX_DEPTH]; MAX_NUM_OF_REF_PIC_LIST],
    /// C `prehme_data`.
    pub prehme_data: [[[SearchInfo; SEARCH_REGION_COUNT]; MAX_REF_IDX]; MAX_NUM_OF_REF_PIC_LIST],
    /// C `prehme_ctrl`.
    pub prehme_ctrl: PreHmeCtrls,
    /// C `x_hme_level0_search_center`.
    pub x_hme_level0_search_center:
        [[[[i16; HME_SA_ROW_MAX]; HME_SA_COL_MAX]; MAX_REF_IDX]; MAX_NUM_OF_REF_PIC_LIST],
    /// C `y_hme_level0_search_center`.
    pub y_hme_level0_search_center:
        [[[[i16; HME_SA_ROW_MAX]; HME_SA_COL_MAX]; MAX_REF_IDX]; MAX_NUM_OF_REF_PIC_LIST],
    /// C `hme_level0_sad`.
    pub hme_level0_sad:
        [[[[u64; HME_SA_ROW_MAX]; HME_SA_COL_MAX]; MAX_REF_IDX]; MAX_NUM_OF_REF_PIC_LIST],
    /// C `x_hme_level1_search_center`.
    pub x_hme_level1_search_center:
        [[[[i16; HME_SA_ROW_MAX]; HME_SA_COL_MAX]; MAX_REF_IDX]; MAX_NUM_OF_REF_PIC_LIST],
    /// C `y_hme_level1_search_center`.
    pub y_hme_level1_search_center:
        [[[[i16; HME_SA_ROW_MAX]; HME_SA_COL_MAX]; MAX_REF_IDX]; MAX_NUM_OF_REF_PIC_LIST],
    /// C `hme_level1_sad`.
    pub hme_level1_sad:
        [[[[u64; HME_SA_ROW_MAX]; HME_SA_COL_MAX]; MAX_REF_IDX]; MAX_NUM_OF_REF_PIC_LIST],
    /// C `x_hme_level2_search_center`.
    pub x_hme_level2_search_center:
        [[[[i16; HME_SA_ROW_MAX]; HME_SA_COL_MAX]; MAX_REF_IDX]; MAX_NUM_OF_REF_PIC_LIST],
    /// C `y_hme_level2_search_center`.
    pub y_hme_level2_search_center:
        [[[[i16; HME_SA_ROW_MAX]; HME_SA_COL_MAX]; MAX_REF_IDX]; MAX_NUM_OF_REF_PIC_LIST],
    /// C `hme_level2_sad`.
    pub hme_level2_sad:
        [[[[u64; HME_SA_ROW_MAX]; HME_SA_COL_MAX]; MAX_REF_IDX]; MAX_NUM_OF_REF_PIC_LIST],
    /// C `me_type`.
    pub me_type: MeType,
    /// C `num_of_list_to_search`.
    pub num_of_list_to_search: u8,
    /// C `num_of_ref_pic_to_search`.
    pub num_of_ref_pic_to_search: [u8; 2],
    /// C `temporal_layer_index`.
    pub temporal_layer_index: u8,
    /// C `is_ref`.
    pub is_ref: bool,
    /// C `tf_me_exit_th` (read by `svt_aom_motion_estimation_b64`).
    pub tf_me_exit_th: u32,
    /// C `tf_use_pred_64x64_only_th` (written by the ME_MCTF early exit).
    pub tf_use_pred_64x64_only_th: u8,
    /// C `tf_tot_vert_blks`.
    pub tf_tot_vert_blks: u32,
    /// C `tf_tot_horz_blks`.
    pub tf_tot_horz_blks: u32,
    /// C `prune_me_candidates_th`.
    pub prune_me_candidates_th: i32,
    /// C `use_best_unipred_cand_only`.
    pub use_best_unipred_cand_only: u8,
    /// C `sc_class_me_boost`.
    pub sc_class_me_boost: u8,
    /// C `reduce_hme_l0_sr_th_min`.
    pub reduce_hme_l0_sr_th_min: u8,
    /// C `reduce_hme_l0_sr_th_max`.
    pub reduce_hme_l0_sr_th_max: u8,
    /// C `zz_sad`.
    pub zz_sad: [[u32; REF_LIST_MAX_DEPTH]; MAX_NUM_OF_REF_PIC_LIST],
    /// C `me_early_exit_th`.
    pub me_early_exit_th: u32,
    /// C `me_static_b64_th`.
    pub me_static_b64_th: u32,
    /// C `me_safe_limit_zz_th`.
    pub me_safe_limit_zz_th: u32,
    /// C `b64_width`.
    pub b64_width: u32,
    /// C `b64_height`.
    pub b64_height: u32,
    /// C `performed_phme`.
    pub performed_phme: [[[u8; 2]; REF_LIST_MAX_DEPTH]; MAX_NUM_OF_REF_PIC_LIST],
    /// C `prev_me_stage_based_exit_th`.
    pub prev_me_stage_based_exit_th: u32,
}

impl Default for MeContext {
    fn default() -> Self {
        Self {
            interpolated_full_stride: [[0; MAX_REF_IDX]; MAX_NUM_OF_REF_PIC_LIST],
            integer_buffer_off: [[0; MAX_REF_IDX]; MAX_NUM_OF_REF_PIC_LIST],
            me_distortion: [0; SQUARE_PU_COUNT],
            p_sad32x32: [0; 4],
            p_sad16x16: [0; 16],
            p_sad8x8: [0; 64],
            p_sb_best_sad: [[[0; SQUARE_PU_COUNT]; MAX_REF_IDX]; MAX_NUM_OF_REF_PIC_LIST],
            p_sb_best_mv: [[[0; SQUARE_PU_COUNT]; MAX_REF_IDX]; MAX_NUM_OF_REF_PIC_LIST],
            p_eight_sad32x32: [[0; 8]; 4],
            p_eight_sad16x16: [[0; 8]; 16],
            hme_search_method: FULL_SAD_SEARCH,
            me_search_method: FULL_SAD_SEARCH,
            enable_hme_flag: false,
            enable_hme_level0_flag: false,
            enable_hme_level1_flag: false,
            enable_hme_level2_flag: false,
            me_hme_prune_ctrls: MeHmeRefPruneCtrls::default(),
            me_sr_adjustment_ctrls: MeSrCtrls::default(),
            me_8x8_var_ctrls: Me8x8VarCtrls::default(),
            mv_based_sa_adj: MvBasedSearchAdj::default(),
            best_list_idx: 0,
            best_ref_idx: 0,
            me_sa: SearchAreaMinMax::default(),
            num_hme_sa_w: 1,
            num_hme_sa_h: 1,
            hme_l0_sa: SearchAreaMinMax::default(),
            hme_l1_sa: SearchArea::default(),
            hme_l2_sa: SearchArea::default(),
            search_results: [[SearchResults::default(); REF_LIST_MAX_DEPTH];
                MAX_NUM_OF_REF_PIC_LIST],
            reduce_me_sr_divisor: [[1; REF_LIST_MAX_DEPTH]; MAX_NUM_OF_REF_PIC_LIST],
            prehme_data: [[[SearchInfo::default(); SEARCH_REGION_COUNT]; MAX_REF_IDX];
                MAX_NUM_OF_REF_PIC_LIST],
            prehme_ctrl: PreHmeCtrls::default(),
            x_hme_level0_search_center: [[[[0; HME_SA_ROW_MAX]; HME_SA_COL_MAX]; MAX_REF_IDX];
                MAX_NUM_OF_REF_PIC_LIST],
            y_hme_level0_search_center: [[[[0; HME_SA_ROW_MAX]; HME_SA_COL_MAX]; MAX_REF_IDX];
                MAX_NUM_OF_REF_PIC_LIST],
            hme_level0_sad: [[[[0; HME_SA_ROW_MAX]; HME_SA_COL_MAX]; MAX_REF_IDX];
                MAX_NUM_OF_REF_PIC_LIST],
            x_hme_level1_search_center: [[[[0; HME_SA_ROW_MAX]; HME_SA_COL_MAX]; MAX_REF_IDX];
                MAX_NUM_OF_REF_PIC_LIST],
            y_hme_level1_search_center: [[[[0; HME_SA_ROW_MAX]; HME_SA_COL_MAX]; MAX_REF_IDX];
                MAX_NUM_OF_REF_PIC_LIST],
            hme_level1_sad: [[[[0; HME_SA_ROW_MAX]; HME_SA_COL_MAX]; MAX_REF_IDX];
                MAX_NUM_OF_REF_PIC_LIST],
            x_hme_level2_search_center: [[[[0; HME_SA_ROW_MAX]; HME_SA_COL_MAX]; MAX_REF_IDX];
                MAX_NUM_OF_REF_PIC_LIST],
            y_hme_level2_search_center: [[[[0; HME_SA_ROW_MAX]; HME_SA_COL_MAX]; MAX_REF_IDX];
                MAX_NUM_OF_REF_PIC_LIST],
            hme_level2_sad: [[[[0; HME_SA_ROW_MAX]; HME_SA_COL_MAX]; MAX_REF_IDX];
                MAX_NUM_OF_REF_PIC_LIST],
            me_type: MeType::OpenLoop,
            num_of_list_to_search: 1,
            num_of_ref_pic_to_search: [1, 0],
            temporal_layer_index: 0,
            is_ref: false,
            tf_me_exit_th: 0,
            tf_use_pred_64x64_only_th: 0,
            tf_tot_vert_blks: 0,
            tf_tot_horz_blks: 0,
            prune_me_candidates_th: 0,
            use_best_unipred_cand_only: 0,
            sc_class_me_boost: 0,
            reduce_hme_l0_sr_th_min: 0,
            reduce_hme_l0_sr_th_max: 0,
            zz_sad: [[u32::MAX; REF_LIST_MAX_DEPTH]; MAX_NUM_OF_REF_PIC_LIST],
            me_early_exit_th: 0,
            me_static_b64_th: 0,
            me_safe_limit_zz_th: 0,
            b64_width: 64,
            b64_height: 64,
            performed_phme: [[[0; 2]; REF_LIST_MAX_DEPTH]; MAX_NUM_OF_REF_PIC_LIST],
            prev_me_stage_based_exit_th: 0,
        }
    }
}

/// The `PictureParentControlSet` / `SequenceControlSet` fields
/// `motion_estimation.c` reads. Everything here is INPUT — ME never writes a
/// picture field except through [`MeB64Output`].
#[derive(Clone, Copy, Debug, Default)]
pub struct MePicParams {
    /// C `pcs->picture_number`.
    pub picture_number: u64,
    /// C `pcs->aligned_width`.
    pub aligned_width: i16,
    /// C `pcs->aligned_height`.
    pub aligned_height: i16,
    /// C `pcs->enhanced_pic->width`.
    pub enhanced_width: u32,
    /// C `pcs->enhanced_pic->height`.
    pub enhanced_height: u32,
    /// C `pcs->ahd_error`.
    pub ahd_error: u32,
    /// C `pcs->input_resolution` / `scs->input_resolution`.
    pub input_resolution: u8,
    /// C `pcs->enable_me_8x8`.
    pub enable_me_8x8: bool,
    /// C `pcs->enable_me_16x16`.
    pub enable_me_16x16: bool,
    /// C `pcs->max_number_of_pus_per_sb`.
    pub max_number_of_pus_per_sb: u8,
    /// C `pcs->hierarchical_levels`.
    pub hierarchical_levels: u8,
    /// C `pcs->similar_brightness_refs`.
    pub similar_brightness_refs: bool,
    /// C `frame_is_boosted(pcs)` (enc_mode_config.h:108).
    pub frame_is_boosted: bool,
    /// C `frame_is_leaf(pcs)` (enc_mode_config.h:113).
    pub frame_is_leaf: bool,
    /// C `pcs->gm_ctrls.enabled`.
    pub gm_enabled: bool,
    /// C `pcs->scs->mrp_ctrls.only_l_bwd`.
    pub only_l_bwd: bool,
    /// C `pcs->pa_me_data->max_cand`.
    pub max_cand: usize,
    /// C `pcs->pa_me_data->max_refs`.
    pub max_refs: usize,
    /// C `pcs->pa_me_data->max_l0`.
    pub max_l0: usize,
    /// C `pcs->b64_geom[b64_index].width`.
    pub b64_geom_width: u32,
    /// C `pcs->b64_geom[b64_index].height`.
    pub b64_geom_height: u32,
    /// C `input_ptr->width` (the source picture desc, NOT the aligned size).
    pub input_width: u16,
    /// C `input_ptr->height`.
    pub input_height: u16,
}

/// Everything `svt_aom_motion_estimation_b64` writes on the picture side for
/// one b64 — C scatters these across `pcs->pa_me_data->me_results[sb_index]`
/// and six per-b64 `pcs->` arrays.
#[derive(Clone, Debug, Default)]
pub struct MeB64Output {
    /// C `me_results[sb]->total_me_candidate_index`, indexed by `pu_index`.
    pub total_me_candidate_index: alloc::vec::Vec<u8>,
    /// C `me_results[sb]->me_candidate_array`, `pu_index * max_cand + cand`.
    pub me_candidate_array: alloc::vec::Vec<MeCandidate>,
    /// C `me_results[sb]->me_mv_array`, `pu_index * max_refs + slot`.
    pub me_mv_array: alloc::vec::Vec<Mv>,
    /// C `pcs->rc_me_allow_gm[b64]`.
    pub rc_me_allow_gm: u8,
    /// C `pcs->rc_me_distortion[b64]`.
    pub rc_me_distortion: u32,
    /// C `pcs->me_8x8_cost_variance[b64]`.
    pub me_8x8_cost_variance: u32,
    /// C `pcs->me_64x64_distortion[b64]`.
    pub me_64x64_distortion: u32,
    /// C `pcs->me_32x32_distortion[b64]`.
    pub me_32x32_distortion: u32,
    /// C `pcs->me_16x16_distortion[b64]`.
    pub me_16x16_distortion: u32,
    /// C `pcs->me_8x8_distortion[b64]`.
    pub me_8x8_distortion: u32,
}

impl MeB64Output {
    /// Allocate the three variable-length arrays the way
    /// `svt_aom_pa_reference_object_ctor` sizes them: one candidate slot per
    /// `(pu, max_cand)`, one MV slot per `(pu, max_refs)`, one count per pu.
    pub fn new(max_cand: usize, max_refs: usize) -> Self {
        let mut me = Self::default();
        me.reset(max_cand, max_refs);
        me
    }

    /// [`new`](Self::new) into an EXISTING allocation.
    ///
    /// C checks a `MeResults` out of a pool built once by
    /// `svt_aom_pa_reference_object_ctor` (`reference_object.c`) and clears
    /// the counts; the port allocated three `Vec`s per b64 per frame
    /// (`MeB64Output::new` 12.53 M over 6,144 calls in
    /// `benchmarks/mem_heaptrack_2026-09-03.txt`). This reproduces `new`'s
    /// state exactly — every element is overwritten with the same value
    /// `new` would have allocated it with, and every scalar is reset to its
    /// `Default` — so the search that follows cannot observe the difference.
    pub fn reset(&mut self, max_cand: usize, max_refs: usize) {
        self.total_me_candidate_index.clear();
        self.total_me_candidate_index.resize(SQUARE_PU_COUNT, 0u8);
        self.me_candidate_array.clear();
        self.me_candidate_array
            .resize(SQUARE_PU_COUNT * max_cand, MeCandidate::default());
        self.me_mv_array.clear();
        self.me_mv_array
            .resize(SQUARE_PU_COUNT * max_refs, Mv::ZERO);
        // The scalar tail of `..Self::default()`.
        let d = Self::default();
        self.rc_me_allow_gm = d.rc_me_allow_gm;
        self.rc_me_distortion = d.rc_me_distortion;
        self.me_8x8_cost_variance = d.me_8x8_cost_variance;
        self.me_64x64_distortion = d.me_64x64_distortion;
        self.me_32x32_distortion = d.me_32x32_distortion;
        self.me_16x16_distortion = d.me_16x16_distortion;
        self.me_8x8_distortion = d.me_8x8_distortion;
    }
}
