use super::*;

/// C `get_block_position` (md_config_process.c:396-419). Returns
/// `Some((mi_r, mi_c))` when the projected position is usable.
pub fn get_block_position(
    mi_rows: i32,
    mi_cols: i32,
    blk_row: i32,
    blk_col: i32,
    mv: Mv,
    sign_bias: bool,
) -> Option<(i32, i32)> {
    let base_blk_row = (blk_row >> 3) << 3;
    let base_blk_col = (blk_col >> 3) << 3;

    // (4 + MI_SIZE_LOG2) == 6.
    let row_offset = if mv.y >= 0 {
        i32::from(mv.y) >> 6
    } else {
        -((-i32::from(mv.y)) >> 6)
    };
    let col_offset = if mv.x >= 0 {
        i32::from(mv.x) >> 6
    } else {
        -((-i32::from(mv.x)) >> 6)
    };

    let row = if sign_bias {
        blk_row - row_offset
    } else {
        blk_row + row_offset
    };
    let col = if sign_bias {
        blk_col - col_offset
    } else {
        blk_col + col_offset
    };

    if row < 0 || row >= (mi_rows >> 1) || col < 0 || col >= (mi_cols >> 1) {
        return None;
    }
    if row < base_blk_row - (MAX_OFFSET_HEIGHT >> 3)
        || row >= base_blk_row + 8 + (MAX_OFFSET_HEIGHT >> 3)
        || col < base_blk_col - (MAX_OFFSET_WIDTH >> 3)
        || col >= base_blk_col + 8 + (MAX_OFFSET_WIDTH >> 3)
    {
        return None;
    }
    Some((row, col))
}

/// One reference frame's saved motion field plus the metadata
/// [`motion_field_projection`] reads off `EbReferenceObject`.
pub struct RefMotionField<'a> {
    /// C `start_frame_buf->mvs`, an `mvs_rows * mvs_cols` grid.
    pub mvs: &'a [MvRef],
    /// C `start_frame_buf->order_hint`.
    pub order_hint: i32,
    /// C `start_frame_buf->ref_order_hint[0..7]`, indexed by
    /// `ref - LAST_FRAME`.
    pub ref_order_hint: [i32; INTER_REFS_PER_FRAME],
    /// C `start_frame_buf->frame_type` being KEY / INTRA_ONLY aborts the
    /// projection.
    pub is_intra_only: bool,
    /// C `start_frame_buf->mi_rows` / `mi_cols` — a mismatch with the
    /// current frame aborts (AV1 spec 7.9.2).
    pub mi_rows: i32,
    pub mi_cols: i32,
}

/// C `motion_field_projection` (md_config_process.c:427-521). Writes into
/// `tpl_mvs` (stride `mi_stride >> 1`) and returns C's `int`.
///
/// `dir` is C's `dir` argument (0 or 2); the sign flip and the
/// `get_block_position` sign bias both derive from it exactly as C does.
pub fn motion_field_projection(
    tpl_mvs: &mut [TplMvRef],
    tpl_stride: i32,
    mi_rows: i32,
    mi_cols: i32,
    cur_order_hint: i32,
    order_hint_info: OrderHintInfo,
    start_frame: Option<&RefMotionField>,
    dir: i32,
) -> i32 {
    let Some(buf) = start_frame else {
        return 0;
    };
    if buf.is_intra_only {
        return 0;
    }
    if buf.mi_rows != mi_rows || buf.mi_cols != mi_cols {
        return 0;
    }

    let start_frame_order_hint = buf.order_hint;
    let mut start_to_current_frame_offset =
        get_relative_dist(order_hint_info, start_frame_order_hint, cur_order_hint);

    let mut ref_offset = [0i32; REF_FRAMES];
    for i in (LAST_FRAME as usize)..=(ALTREF_FRAME as usize) {
        ref_offset[i] = get_relative_dist(
            order_hint_info,
            start_frame_order_hint,
            buf.ref_order_hint[i - LAST_FRAME as usize],
        );
    }

    if dir == 2 {
        start_to_current_frame_offset = -start_to_current_frame_offset;
    }

    let mvs_rows = (mi_rows + 1) >> 1;
    let mvs_cols = (mi_cols + 1) >> 1;

    for blk_row in 0..mvs_rows {
        for blk_col in 0..mvs_cols {
            let mv_ref = buf.mvs[(blk_row * mvs_cols + blk_col) as usize];
            let fwd_mv = mv_ref.mv;

            if mv_ref.ref_frame > INTRA_FRAME {
                let ref_frame_offset = ref_offset[mv_ref.ref_frame as usize];

                let pos_valid = ref_frame_offset.abs() <= MAX_FRAME_DISTANCE
                    && ref_frame_offset > 0
                    && start_to_current_frame_offset.abs() <= MAX_FRAME_DISTANCE;

                if pos_valid {
                    let this_mv =
                        get_mv_projection(fwd_mv, start_to_current_frame_offset, ref_frame_offset);
                    if let Some((mi_r, mi_c)) = get_block_position(
                        mi_rows,
                        mi_cols,
                        blk_row,
                        blk_col,
                        this_mv,
                        (dir >> 1) != 0,
                    ) {
                        let mi_offset = mi_r * tpl_stride + mi_c;
                        tpl_mvs[mi_offset as usize].mfmv0 = fwd_mv;
                        tpl_mvs[mi_offset as usize].ref_frame_offset = ref_frame_offset as u8;
                    }
                }
            }
        }
    }

    1
}

/// The per-reference inputs [`setup_motion_field`] needs, indexed by
/// `ref - LAST_FRAME` (0..7).
pub struct MotionFieldRefs<'a> {
    pub refs: [Option<RefMotionField<'a>>; INTER_REFS_PER_FRAME],
}

/// What [`setup_motion_field`] produces besides the filled `tpl_mvs`:
/// C's `pcs->ref_frame_side[TOTAL_REFS_PER_FRAME]`.
pub type RefFrameSide = [i8; TOTAL_REFS_PER_FRAME];

/// C `av1_setup_motion_field` (md_config_process.c:523-580).
///
/// Fills `tpl_mvs` (which the caller sizes
/// `((mi_rows + MAX_MIB_SIZE) >> 1) * (mi_stride >> 1)`, as C does) and
/// returns `ref_frame_side`. Note C computes `ref_frame_side`
/// unconditionally but returns EARLY — before touching `tpl_mvs` — when
/// `use_ref_frame_mvs` is 0; that early return is reproduced.
pub fn setup_motion_field(
    tpl_mvs: &mut [TplMvRef],
    tpl_stride: i32,
    mi_rows: i32,
    mi_cols: i32,
    cur_order_hint: i32,
    order_hint_info: OrderHintInfo,
    use_ref_frame_mvs: bool,
    refs: &MotionFieldRefs,
) -> RefFrameSide {
    let mut ref_frame_side: RefFrameSide = [0; TOTAL_REFS_PER_FRAME];
    if !order_hint_info.enable_order_hint {
        return ref_frame_side;
    }

    let mut ref_order_hint = [0i32; INTER_REFS_PER_FRAME];
    for ref_frame in (LAST_FRAME as usize)..=(ALTREF_FRAME as usize) {
        let ref_idx = ref_frame - LAST_FRAME as usize;
        let order_hint = refs.refs[ref_idx].as_ref().map_or(0, |b| b.order_hint);
        ref_order_hint[ref_idx] = order_hint;
        if get_relative_dist(order_hint_info, order_hint, cur_order_hint) > 0 {
            ref_frame_side[ref_frame] = 1;
        } else if order_hint == cur_order_hint {
            ref_frame_side[ref_frame] = -1;
        }
    }

    if !use_ref_frame_mvs {
        return ref_frame_side;
    }

    for slot in tpl_mvs.iter_mut() {
        *slot = TplMvRef::default();
    }

    let project = |tpl: &mut [TplMvRef], start: i8, dir: i32| -> i32 {
        motion_field_projection(
            tpl,
            tpl_stride,
            mi_rows,
            mi_cols,
            cur_order_hint,
            order_hint_info,
            refs.refs[(start - LAST_FRAME) as usize].as_ref(),
            dir,
        )
    };

    let mut ref_stamp = MFMV_STACK_SIZE - 1;
    if refs.refs[0].is_some() {
        let alt_of_lst_order_hint = refs.refs[0].as_ref().map_or(0, |b| {
            b.ref_order_hint[(ALTREF_FRAME - LAST_FRAME) as usize]
        });
        let is_lst_overlay =
            alt_of_lst_order_hint == ref_order_hint[(GOLDEN_FRAME - LAST_FRAME) as usize];
        if !is_lst_overlay {
            project(tpl_mvs, LAST_FRAME, 2);
        }
        ref_stamp -= 1;
    }

    if get_relative_dist(
        order_hint_info,
        ref_order_hint[(BWDREF_FRAME - LAST_FRAME) as usize],
        cur_order_hint,
    ) > 0
        && project(tpl_mvs, BWDREF_FRAME, 0) != 0
    {
        ref_stamp -= 1;
    }

    if get_relative_dist(
        order_hint_info,
        ref_order_hint[(ALTREF2_FRAME - LAST_FRAME) as usize],
        cur_order_hint,
    ) > 0
        && project(tpl_mvs, ALTREF2_FRAME, 0) != 0
    {
        ref_stamp -= 1;
    }

    if get_relative_dist(
        order_hint_info,
        ref_order_hint[(ALTREF_FRAME - LAST_FRAME) as usize],
        cur_order_hint,
    ) > 0
        && ref_stamp >= 0
        && project(tpl_mvs, ALTREF_FRAME, 0) != 0
    {
        ref_stamp -= 1;
    }

    if ref_stamp >= 0 {
        project(tpl_mvs, LAST2_FRAME, 2);
    }

    ref_frame_side
}

// ---------------------------------------------------------------------------
// Compound mode-context collapse (inter_prediction.c:2565-2581)
// ---------------------------------------------------------------------------
