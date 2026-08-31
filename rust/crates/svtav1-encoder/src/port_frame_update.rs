//! Frame update-type classification — port of `svt_aom_get_frame_update_type`
//! (`Codec/resize.c:1246`).
//!
//! The C function answers "what ROLE does this frame play in the GOP?" and two
//! very different consumers read the answer:
//!
//! * `svt_aom_compute_rd_mult_based_on_qindex(bit_depth, update_type, qindex)`
//!   — the per-frame RD lambda. `rd_frame_type_factor[..][update_type]` is a
//!   different multiplier per update type, so an inter frame priced with
//!   `KF_UPDATE` gets the KEY-frame lambda and every RD decision below it is
//!   wrong.
//! * `temporal_filtering.c:2739` — the temporal-filter strength arm.
//!
//! The port has so far hardcoded `KF_UPDATE` (see `pd0.rs`, which says so),
//! which is correct for the still-picture envelope and wrong for every inter
//! frame. This module is the classifier itself; wiring it into the lambda is
//! the caller's job.
//!
//! Evidence: **tier 1** — `tests/c_parity_frame_update.rs` drives the real
//! exported `svt_aom_get_frame_update_type` through `svtav1-cref` over the
//! whole reachable `(frame_type, hierarchical_levels, temporal_layer_index)`
//! domain.

use svtav1_types::frame::FrameType;

/// `SvtAv1FrameUpdateType` (`API/EbSvtAv1Enc.h:183`).
///
/// The discriminants are the C enum's, because they index
/// `rd_frame_type_factor[]` and are compared against directly all over the C
/// tree. Only four of the seven are ever produced by
/// `svt_aom_get_frame_update_type`; the overlay types come from the
/// rate-control GF group, which this function deliberately does not consult
/// (see its own comment: `gf_group->update_type` is valid only in the 2nd pass
/// of a 2-pass encode or with `lap_rc`, and is set in the RC process, so it
/// cannot be used by processes that run before RC).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum FrameUpdateType {
    /// `SVT_AV1_KF_UPDATE`
    Kf = 0,
    /// `SVT_AV1_LF_UPDATE`
    Lf = 1,
    /// `SVT_AV1_GF_UPDATE` — never returned by `frame_update_type`.
    Gf = 2,
    /// `SVT_AV1_ARF_UPDATE`
    Arf = 3,
    /// `SVT_AV1_OVERLAY_UPDATE` — never returned by `frame_update_type`.
    Overlay = 4,
    /// `SVT_AV1_INTNL_OVERLAY_UPDATE` — never returned by `frame_update_type`.
    IntnlOverlay = 5,
    /// `SVT_AV1_INTNL_ARF_UPDATE`
    IntnlArf = 6,
}

impl FrameUpdateType {
    /// The raw C discriminant, for handing to
    /// `compute_rd_mult_based_on_qindex` and friends.
    #[inline]
    pub const fn as_i32(self) -> i32 {
        self as i32
    }
}

/// Port of `svt_aom_get_frame_update_type` (`Codec/resize.c:1246`).
///
/// ```text
/// if (frm_hdr.frame_type == KEY_FRAME)          return KF_UPDATE;
/// if (hierarchical_levels > 0) {
///     if (temporal_layer_index == 0)                    return ARF_UPDATE;
///     if (temporal_layer_index == hierarchical_levels)  return LF_UPDATE;
///     return INTNL_ARF_UPDATE;
/// }
/// return LF_UPDATE;
/// ```
///
/// Two details that are easy to get wrong and are transcribed literally:
///
/// * the KEY test is on `frame_type == KEY_FRAME` **only** — an
///   `INTRA_ONLY_FRAME` is *not* a `KF_UPDATE`, even though it is intra. C
///   compares the enum, not `frame_is_intra_only`.
/// * the last-layer test is `==`, not `>=`. `temporal_layer_index` never
///   exceeds `hierarchical_levels` in a well-formed GOP, so the two agree in
///   practice, but the port keeps C's comparison so a malformed input maps the
///   same way C maps it.
#[inline]
pub fn frame_update_type(
    frame_type: FrameType,
    hierarchical_levels: u8,
    temporal_layer_index: u8,
) -> FrameUpdateType {
    if frame_type == FrameType::Key {
        return FrameUpdateType::Kf;
    }
    if hierarchical_levels > 0 {
        if temporal_layer_index == 0 {
            FrameUpdateType::Arf
        } else if temporal_layer_index == hierarchical_levels {
            FrameUpdateType::Lf
        } else {
            FrameUpdateType::IntnlArf
        }
    } else {
        FrameUpdateType::Lf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_frame_is_kf_update_at_every_layer() {
        for hier in 0..=6u8 {
            for tl in 0..=hier {
                assert_eq!(
                    frame_update_type(FrameType::Key, hier, tl),
                    FrameUpdateType::Kf
                );
            }
        }
    }

    #[test]
    fn flat_gop_is_always_lf() {
        for tl in 0..=6u8 {
            assert_eq!(
                frame_update_type(FrameType::Inter, 0, tl),
                FrameUpdateType::Lf
            );
        }
    }

    #[test]
    fn base_layer_is_arf_and_top_layer_is_lf() {
        // The default 5-layer GOP: base = ARF, top = LF, middle = INTNL_ARF.
        assert_eq!(
            frame_update_type(FrameType::Inter, 4, 0),
            FrameUpdateType::Arf
        );
        assert_eq!(
            frame_update_type(FrameType::Inter, 4, 4),
            FrameUpdateType::Lf
        );
        for tl in 1..4u8 {
            assert_eq!(
                frame_update_type(FrameType::Inter, 4, tl),
                FrameUpdateType::IntnlArf
            );
        }
    }

    #[test]
    fn intra_only_is_not_a_kf_update() {
        // C tests `frame_type == KEY_FRAME`, not `frame_is_intra_only`.
        assert_eq!(
            frame_update_type(FrameType::IntraOnly, 4, 0),
            FrameUpdateType::Arf
        );
    }
}
