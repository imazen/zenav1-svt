//! Wholesale port of `Source/Lib/Codec/enc_mode_config.c` — the per-preset
//! signal derivations.
//!
//! # Which arm is live
//!
//! `enc_mode_config.c` carries three parallel arms for almost every signal:
//! `*_allintra`, `*_rtc` and `*_default`. `scs->allintra` is
//! `(intra_period_length == 0 || avif || pred_structure == ALL_INTRA)`
//! (`Globals/enc_handle.c:4406`, re-evaluated at `:4704`), and `rtc` is
//! `static_config.rtc`. The still/AVIF envelope this port shipped first is the
//! **allintra** arm; a video-mode encode (`SVT_AVIF=0`, nonzero intra period)
//! takes the **default** arm. Both must exist side by side — the default arm
//! does not replace the allintra one.
//!
//! Before this module the port carried the *resolved allintra constants* per
//! preset, inlined in `leaf_funnel/rate_tables.rs` (`FunnelCfg::for_preset`),
//! `pd0.rs`, `depth_refine.rs` and `speed_config.rs` — there were no Rust
//! functions corresponding to these C ones at all. So "porting X" here means
//! introducing the `level -> controls` table the port had flattened.
//!
//! # Types
//!
//! `enc_mode` is C's `EncMode`, an `int8_t`-ranged enum whose *first* value is
//! `ENC_MR = -1` (`API/EbSvtAv1Enc.h:47`), so it is modelled as `i8` and every
//! `enc_mode <= ENC_MR` predicate is `enc_mode <= -1`. `SpeedConfig::preset`
//! is a `u8`, so MR is structurally unreachable from the port's public API
//! (`rust/CLAUDE.md` envelope guard 5) — the MR arms are translated anyway
//! per `WORKING-ON-THIS.md` §7 (dead-looking C stays translated).
//!
//! # Evidence
//!
//! Every exported C symbol reachable here is gated at **tier 1** (a
//! differential against the real symbol in `libSvtAv1Enc.a` via
//! `svtav1-cref`). File-`static` C functions are gated at **tier 4**
//! (hand-derived vectors traced against the C source) and say so at the test.

pub mod common;
pub mod ctrls;
pub mod encdec;
pub mod leaf;
pub mod light_pd1;
pub mod me;
pub mod pd0;
pub mod tail;

/// C `ResolutionRange` (`Codec/definitions.h:1822`).
///
/// Ordering is load-bearing: the C predicates are `<=`/`>=` comparisons on the
/// enum's integer value, so the discriminants must match exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum ResolutionRange {
    /// `INPUT_SIZE_240p_RANGE`
    R240p = 0,
    /// `INPUT_SIZE_360p_RANGE`
    R360p = 1,
    /// `INPUT_SIZE_480p_RANGE`
    R480p = 2,
    /// `INPUT_SIZE_720p_RANGE`
    R720p = 3,
    /// `INPUT_SIZE_1080p_RANGE`
    R1080p = 4,
    /// `INPUT_SIZE_4K_RANGE`
    R4k = 5,
    /// `INPUT_SIZE_8K_RANGE`
    R8k = 6,
}

impl ResolutionRange {
    /// The enum's integer value, which is what every C predicate compares.
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// C `svt_aom_derive_input_resolution` (`Codec/utility.c`): map a luma
    /// pixel count to its range bucket. Thresholds are
    /// `Codec/definitions.h:1833-1839`.
    #[must_use]
    pub const fn from_luma_area(area: u32) -> Self {
        // C: `input_size < INPUT_SIZE_<N>_TH` walking upward.
        if area < 0x28500 {
            Self::R240p
        } else if area < 0x4CE00 {
            Self::R360p
        } else if area < 0xA1400 {
            Self::R480p
        } else if area < 0x16DA00 {
            Self::R720p
        } else if area < 0x535200 {
            Self::R1080p
        } else if area < 0x140A000 {
            Self::R4k
        } else {
            Self::R8k
        }
    }
}

/// C `InputCoeffLvl` (`Codec/definitions.h:283`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum InputCoeffLvl {
    /// `VLOW_LVL`
    VLow = 0,
    /// `LOW_LVL`
    Low = 1,
    /// `NORMAL_LVL`
    Normal = 2,
    /// `HIGH_LVL`
    High = 3,
}

/// C `EncMode` values used as `<=` bounds. `ENC_MR` is `-1`.
pub mod enc_mode {
    /// `ENC_MR = -1` — research mode, above M0 in quality.
    pub const MR: i8 = -1;
    /// `ENC_M0`
    pub const M0: i8 = 0;
    /// `ENC_M1`
    pub const M1: i8 = 1;
    /// `ENC_M2`
    pub const M2: i8 = 2;
    /// `ENC_M3`
    pub const M3: i8 = 3;
    /// `ENC_M4`
    pub const M4: i8 = 4;
    /// `ENC_M5`
    pub const M5: i8 = 5;
    /// `ENC_M6`
    pub const M6: i8 = 6;
    /// `ENC_M7`
    pub const M7: i8 = 7;
    /// `ENC_M8`
    pub const M8: i8 = 8;
    /// `ENC_M9`
    pub const M9: i8 = 9;
    /// `ENC_M10`
    pub const M10: i8 = 10;
    /// `ENC_M11`
    pub const M11: i8 = 11;
    /// `ENC_M12`
    pub const M12: i8 = 12;
    /// `ENC_M13`
    pub const M13: i8 = 13;
}
