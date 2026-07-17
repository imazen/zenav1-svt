//! 10-bit (bd10) — chunk-1 scaffolding (task #94, inert for bd8).
//!
//! Source translation per docs/bd10-port-map.md. UNWIRED (add
//! `pub mod bd10;` when integration starts); bulk-write directive
//! 2026-07-17, no build run yet.
//!
//! Design decisions locked by the map:
//! - Plain u16 planes everywhere; the C 8+2 split is input-buffer memory
//!   layout ONLY — never ported.
//! - PD_PASS_0 is unconditionally 8-bit at every preset: pd0.rs keeps its
//!   u8 path, reading the MSB-TRUNCATED plane built at ingestion.
//! - Our target presets (M0+) run hbd_md = DUAL == true 10-bit for all
//!   intra work (the 8-bit downgrade exists only in IntraBC
//!   compensation); MR-tier = true 10-bit outright.
//! - Precision is RD-VISIBLE: wherever C reads the truncated plane with
//!   8-bit lambdas (PD0, hbd_md=0 pockets, the sc detector), the port
//!   must do exactly that — "more precision" changes the bitstream.

/// C clip_pixel_highbd (definitions.h:725-734).
#[inline]
pub fn clip_pixel_highbd(v: i32, bd: u8) -> u16 {
    let max = (1i32 << bd) - 1;
    v.clamp(0, max) as u16
}

/// The MSB-truncation C applies when a 10-bit source feeds an 8-bit
/// consumer (svt_unpack_and_2bcompress: out8 = px >> 2, NO rounding).
/// Consumers: PD0's plane, the sc detector, any hbd_md=0 pocket.
#[inline]
pub fn msb_truncate_plane(src10: &[u16], dst8: &mut [u8]) {
    for (d, &s) in dst8.iter_mut().zip(src10.iter()) {
        *d = (s >> 2) as u8;
    }
}

/// Lambda scaling at bd10 (C md_process.c:753-754): the SSD-domain full
/// lambda gets *16 (2^(2*(10-8))) and the SAD-domain fast lambda *4.
/// Applied ON TOP of the qindex->rdmult derivation, which itself does
/// ROUND_POWER_OF_TWO(rdmult, 4) at EB_TEN_BIT (rc_process.c:365-393).
/// PORT-NOTE(unverified): verify both stages against a C lambda dump at
/// the first bd10 cell — the two shifts are easy to double- or
/// single-apply by mistake.
pub const FULL_LAMBDA_BD10_MULT: u64 = 16;
pub const FAST_LAMBDA_BD10_MULT: u64 = 4;

/// C svt_aom_get_qzbin_factor (inv_transforms.c:3492-3505): the zbin
/// oddness threshold ladder is x4 per 2 bits of depth — 148 (bd8),
/// 592 (bd10), 2368 (bd12); factor 64 vs 84 by dc-quant magnitude.
pub fn qzbin_factor(dc_quant_q3: i32, bd: u8) -> i32 {
    let th = match bd {
        8 => 148,
        10 => 592,
        _ => 2368,
    };
    if dc_quant_q3 < th {
        84
    } else {
        64
    }
}

/// Inverse-transform range check bounds (C check_range/HIGHBD_WRAPLOW,
/// inv_transforms.c:2426-2441): int_max = (1<<(7+bd)) - 1 + (914<<(bd-7)).
pub fn inv_txfm_range_max(bd: u8) -> i32 {
    (1i32 << (7 + bd)) - 1 + (914i32 << (bd - 7))
}

/// dc/ac qlookup tables for bd10 (C dc_qlookup_10_QTX / ac_qlookup_10_QTX,
/// inv_transforms.c:3373-3506; 256 entries each, qindex domain unchanged
/// 0..255 at every bd).
///
/// PORT-NOTE(unverified): VALUES NOT TRANSCRIBED YET — generate with
/// `python3 xtask/transcribe_bd10_qlookup.py` (written alongside this
/// module; reads the C tables, emits `bd10_qlookup_tables.rs` with
/// `pub static DC_QLOOKUP_10: [i16; 256]` / `AC_QLOOKUP_10`), then
/// replace this placeholder with `include!("bd10_qlookup_tables.rs")`.
/// Kept a compile-error-free placeholder so the module is wire-able:
pub fn dc_qlookup_10(_qindex: u8) -> i16 {
    unimplemented!("run xtask/transcribe_bd10_qlookup.py first (PORT-NOTE above)")
}
pub fn ac_qlookup_10(_qindex: u8) -> i16 {
    unimplemented!("run xtask/transcribe_bd10_qlookup.py first (PORT-NOTE above)")
}

/// hbd_md resolution for the allintra path (C enc_mode_config.c:2476-2483
/// + enable_hbd_mode_decision = bd>8 ? DEFAULT : 0). Returns the MD
/// precision the port must replicate per preset. 0 = 8-bit MD (bd8
/// streams), 1 = true 10-bit, 2 = DUAL (== 1 for intra; IntraBC
/// compensation searches at 8-bit).
/// --hbd-mds is NEVER consulted on the allintra path (map §2).
pub fn allintra_hbd_md(encoder_bit_depth: u8, _preset: u8) -> u8 {
    if encoder_bit_depth <= 8 {
        0
    } else {
        // MR-tier (preset < 0 in C's enum) would be 1; our u8 preset
        // surface starts at M0 -> DUAL for every reachable preset.
        2
    }
}
