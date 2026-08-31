//! `write_frame_size_with_refs` and its DPB order-hint lookup — the inter
//! frame header's s-frame / frame-resize arm.
//!
//! C reference: `Source/Lib/Codec/entropy_coding.c` (`get_ref_order_hint`
//! :3230, `write_frame_size_with_refs` :3238), reached from :3526 when
//! `frame_size_override_flag` is set and `error_resilient_mode` is off.
//!
//! # Why the two sub-writers are parameters
//!
//! C's found-arm calls `write_superres_scale` and its not-found arm calls
//! `write_frame_size(pcs, 1, wb)`. Both live in `entropy_coding.c` outside
//! this lane's queue and both already have counterparts in
//! `entropy/obu.rs`, which this lane does not own. Taking them as closures
//! keeps ONE implementation of each in the tree instead of a second, silently
//! diverging copy — the wiring chunk passes `obu.rs`'s.

use crate::entropy::obu::BitWriter;

/// C `INVALID_IDX` (definitions.h:1653), as [`get_ref_order_hint`] returns it.
///
/// C's `get_ref_order_hint` is declared `uint32_t` but returns the literal
/// `INVALID_IDX` (`-1`), and its caller tests `(int32_t)ref_order_hint !=
/// INVALID_IDX`. So the sentinel really is `0xFFFF_FFFF` in the returned
/// `uint32_t`, and the port reproduces that rather than "fixing" the type.
pub const INVALID_ORDER_HINT: u32 = -1i32 as u32;

/// One reference picture as the found-test reads it.
#[derive(Clone, Copy, Debug, Default)]
pub struct RefPic {
    /// C `EbReferenceObject::order_hint`.
    pub order_hint: u32,
    /// C `ref->reference_picture->width`.
    pub width: u32,
    /// C `ref->reference_picture->height`.
    pub height: u32,
}

/// The picture-level state `write_frame_size_with_refs` reads.
#[derive(Clone, Copy, Debug)]
pub struct FrameSizeRefs<'a> {
    /// C `pcs->av1_ref_signal.ref_dpb_index[ref_frame - LAST_FRAME]`, indexed
    /// LAST, LAST2, LAST3, GOLDEN, BWDREF, ALTREF2, ALTREF. `-1`
    /// (`INVALID_IDX`) marks an absent reference.
    pub ref_dpb_index: [i32; 7],
    /// C `pcs->dpb_order_hint[slot]`.
    pub dpb_order_hint: [u32; 8],
    /// C `pcs->child_pcs->ref_pic_ptr_array[REF_LIST_0]`, truncated to
    /// `pcs->ref_list0_count`.
    pub list0: &'a [RefPic],
    /// The same for `REF_LIST_1` / `ref_list1_count`.
    pub list1: &'a [RefPic],
    /// C `pcs->enhanced_pic->width`.
    pub cur_width: u32,
    /// C `pcs->enhanced_pic->height`.
    pub cur_height: u32,
}

/// C `get_ref_order_hint` (entropy_coding.c:3230).
pub fn get_ref_order_hint(refs: &FrameSizeRefs<'_>, ref_frame: i8) -> u32 {
    let slot = refs.ref_dpb_index[(ref_frame - 1) as usize];
    if slot == -1 {
        return INVALID_ORDER_HINT;
    }
    refs.dpb_order_hint[slot as usize]
}

/// C `write_frame_size_with_refs` (entropy_coding.c:3238).
///
/// Walks LAST..ALTREF writing a `found` bit per reference; the FIRST match
/// short-circuits the whole loop (it writes the superres scale and returns),
/// so at most one `1` bit is ever emitted. A reference matches when its
/// order hint equals this frame's DPB hint for that slot AND its dimensions
/// equal the current picture's — list 0 is searched first, then list 1.
///
/// If nothing matches, all seven bits are `0` and the explicit frame size
/// follows with `frame_size_override = 1` (C hardcodes that: the arm is only
/// reached when the override flag is already set).
pub fn write_frame_size_with_refs(
    wb: &mut BitWriter,
    refs: &FrameSizeRefs<'_>,
    write_superres_scale: impl FnOnce(&mut BitWriter),
    write_frame_size_override: impl FnOnce(&mut BitWriter),
) {
    for ref_frame in 1i8..=7 {
        let mut found = false;
        let ref_order_hint = get_ref_order_hint(refs, ref_frame);
        if ref_order_hint != INVALID_ORDER_HINT {
            for r in refs.list0 {
                if r.order_hint != ref_order_hint {
                    continue;
                }
                // Both the superres-upscaled size and the render size should
                // be checked per spec 5.9.7, but SVT fixes
                // render_and_frame_size_different to 0 (see write_render_size),
                // so only the picture dimensions are compared — C's own note.
                found = refs.cur_width == r.width && refs.cur_height == r.height;
                if found {
                    break;
                }
            }
            if !found {
                for r in refs.list1 {
                    if r.order_hint != ref_order_hint {
                        continue;
                    }
                    found = refs.cur_width == r.width && refs.cur_height == r.height;
                    if found {
                        break;
                    }
                }
            }
        }

        wb.write_bit(found);
        if found {
            write_superres_scale(wb);
            return;
        }
    }

    // Not found: C passes a literal `frame_size_override = 1` here.
    write_frame_size_override(wb);
}
