//! Hand-derived vectors for the `static` C functions in `Codec/pd_process.c`
//! — evidence **tier 4** (`docs/WORKING-ON-THIS.md` §4), the weakest tier,
//! used here because these functions have NO exported symbol.
//!
//! Verified with `nm -g Bin/Release/libSvtAv1Enc.a`: of the functions this
//! file covers, none is a global (`T`) symbol. Two of them
//! (`set_all_ref_frame_type`, `set_ref_list_counts`) survive in
//! `cbuild-static/.../pd_process.c.o` as LOCAL (`t`) symbols, so promoting
//! them with `llvm-objcopy --globalize-symbol` on a private copy of that
//! object is a plausible route to tier 1. That route is NOT taken here and is
//! NOT claimed to work — it has not been made to link.
//!
//! Every expectation below is derived by reading the C, step by step, with
//! the derivation written out in the comment above it. Where a value is
//! surprising, the surprise is stated — that is the only defence a tier-4
//! vector has against being a second transcription of the same mistake.

use svtav1_encoder::inter_mvp::OrderHintInfo;
use svtav1_encoder::port_picstruct as pp;
use svtav1_encoder::port_picstruct::{ALT, ALT2, BWD, GOLD, LAST, LAST2, LAST3};

/// The configuration of the inter campaign's first cell
/// (`tools/identity_diff_inter.sh`): low-delay P, flat, CQP.
fn ld_flat_cqp_seq() -> pp::SeqPicParams {
    pp::SeqPicParams {
        pred_structure: pp::PredStructure::LowDelay,
        rate_control_mode: pp::RcMode::CqpOrCrf,
        rtc: false,
        allintra: false,
        mrp_ctrls: pp::MrpCtrls::default(),
        order_hint_info: OrderHintInfo {
            enable_order_hint: true,
            order_hint_bits: 7,
        },
        hierarchical_levels: 0,
        max_managed_refs: 0,
        ..Default::default()
    }
}

fn key_frame(poc: u64) -> pp::PicParams {
    pp::PicParams {
        picture_number: poc,
        decode_order: poc,
        slice_type: pp::SliceType::I,
        is_key_frame: true,
        is_intra_only: true,
        temporal_layer_index: 0,
        hierarchical_levels: 0,
        pred_struct_type: pp::PredStructure::LowDelay,
        aligned_width: 64,
        aligned_height: 64,
        ..Default::default()
    }
}

fn inter_frame(poc: u64, last_idr: u64) -> pp::PicParams {
    pp::PicParams {
        picture_number: poc,
        decode_order: poc,
        slice_type: pp::SliceType::B,
        is_key_frame: false,
        is_intra_only: false,
        temporal_layer_index: 0,
        hierarchical_levels: 0,
        pred_struct_type: pp::PredStructure::LowDelay,
        frame_offset: poc - last_idr,
        aligned_width: 64,
        aligned_height: 64,
        ..Default::default()
    }
}

mod refs;
mod mini_gop;
mod tpl_primary;

mod scene_tf;
