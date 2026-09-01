//! Leaf per-preset getters from `Codec/enc_mode_config.c`.
//!
//! These are the pure `(enc_mode, resolution, flags) -> level` functions the
//! larger signal derivations read. Each carries its C line number; the
//! `_default` arm is the video-mode one (see the module docs for which arm is
//! live).

use super::{InputCoeffLvl, ResolutionRange, enc_mode::*};

/// C `EB_EIGHT_BIT` (`API/EbSvtAv1Enc.h`).
pub const EB_EIGHT_BIT: u8 = 8;

/// C `MAX_INTRA_LEVEL` (`Codec/enc_mode_config.c`, used by the intra-mode
/// level getters), `enc_mode_config.c:21`.
pub const MAX_INTRA_LEVEL: u32 = 10;

/// C `MAX_PD0_LVL` (`enc_mode_config.c:19`) — the PD0-level cap.
pub const MAX_PD0_LVL: u8 = 8;

// ---------------------------------------------------------------------------
// ME enables
// ---------------------------------------------------------------------------

/// C `svt_aom_get_enable_me_8x8` (`enc_mode_config.c:145`). EXPORTED.
///
/// C's own comment warns that without 8x8 ME data an 8x8 block reuses its
/// 16x16 parent's MV, which is a different search result — so this is
/// bit-affecting, not a memory knob.
#[must_use]
pub fn get_enable_me_8x8(enc_mode: i8, input_resolution: ResolutionRange, rtc_tune: bool) -> u8 {
    // C's shape is nested (an `enc_mode` ladder with a resolution test inside
    // the M8 arm), kept as-is so a future upstream edit to either level lands
    // in the same place.
    if rtc_tune {
        if enc_mode <= M8 {
            u8::from(input_resolution <= ResolutionRange::R720p)
        } else {
            0
        }
    } else if enc_mode <= M5 {
        1
    } else if enc_mode <= M8 {
        u8::from(input_resolution <= ResolutionRange::R720p)
    } else {
        0
    }
}

/// C `svt_aom_get_enable_me_16x16` (`enc_mode_config.c:174`). EXPORTED.
///
/// Unconditionally 1; `enc_mode` is `UNUSED`.
#[must_use]
pub fn get_enable_me_16x16(_enc_mode: i8) -> u8 {
    1
}

// ---------------------------------------------------------------------------
// Global motion
// ---------------------------------------------------------------------------

/// C `svt_aom_get_gm_core_level` (`enc_mode_config.c:180`). EXPORTED.
#[must_use]
pub fn get_gm_core_level(enc_mode: i8, super_res_off: bool) -> u8 {
    if !super_res_off {
        return 0;
    }
    if enc_mode <= MR {
        2
    } else if enc_mode <= M4 {
        4
    } else {
        0
    }
}

/// C `svt_aom_derive_gm_level` (`enc_mode_config.c:194`). EXPORTED.
///
/// GM is off on every I-slice, so this returns 0 for the whole still envelope.
#[must_use]
pub fn derive_gm_level(enc_mode: i8, is_islice: bool, super_res_off: bool) -> u8 {
    if is_islice {
        0
    } else {
        get_gm_core_level(enc_mode, super_res_off)
    }
}

// ---------------------------------------------------------------------------
// Candidate-count cap
// ---------------------------------------------------------------------------

/// C `svt_aom_get_max_can_count` (`enc_mode_config.c:1921`). EXPORTED.
///
/// C's comment calls this "a memory feature", but `INC_MD_CAND_CNT`
/// (`mode_decision.c:61`) stops injecting at the cap, so at low presets — where
/// inter injection can actually reach 1225/1000/720 — it truncates the
/// candidate list and is bit-affecting.
#[must_use]
pub fn get_max_can_count(enc_mode: i8, rtc: bool) -> u16 {
    if rtc {
        if enc_mode <= M7 {
            150
        } else if enc_mode <= M8 {
            75
        } else if enc_mode <= M10 {
            65
        } else if enc_mode <= M11 {
            15
        } else {
            10
        }
    } else if enc_mode <= M1 {
        1225
    } else if enc_mode <= M2 {
        1000
    } else if enc_mode <= M3 {
        720
    } else if enc_mode <= M4 {
        576
    } else if enc_mode <= M5 {
        369
    } else if enc_mode <= M6 {
        236
    } else if enc_mode <= M9 {
        190
    } else {
        80
    }
}

// ---------------------------------------------------------------------------
// Block-size disallows
// ---------------------------------------------------------------------------

/// C `dimensions_require_8x8` (`enc_mode_config.c:2947`). static — tier 4.
#[must_use]
pub fn dimensions_require_8x8(aligned_width: u16, aligned_height: u16) -> bool {
    // Start at 64 because 128x128 is not used for an I-slice.
    let start_bsize: u16 = 64;
    let mut leftover_width = aligned_width % start_bsize;
    let mut leftover_height = aligned_height % start_bsize;
    let mut half_bsize = start_bsize >> 1;
    while half_bsize >= 8 {
        if (leftover_width == 0 || leftover_width > half_bsize)
            && (leftover_height == 0 || leftover_height > half_bsize)
        {
            return false;
        }
        leftover_width %= half_bsize;
        leftover_height %= half_bsize;
        half_bsize >>= 1;
    }
    true
}

/// C `svt_aom_get_disallow_4x4_default` (`enc_mode_config.c:8169`). EXPORTED.
///
/// The video arm forbids 4x4 from M3 up; its allintra twin below keeps 4x4
/// through M3. That ONE-preset gap is the whole of `diag 72x88 q40 p3`.
#[must_use]
pub fn get_disallow_4x4_default(enc_mode: i8) -> bool {
    enc_mode > M2
}

/// C `svt_aom_get_disallow_4x4_rtc` (`enc_mode_config.c:8177`). EXPORTED.
#[must_use]
pub fn get_disallow_4x4_rtc() -> bool {
    true
}

/// C `svt_aom_get_disallow_4x4_allintra` (`enc_mode_config.c:8181`). EXPORTED.
#[must_use]
pub fn get_disallow_4x4_allintra(enc_mode: i8) -> bool {
    enc_mode > M3
}

/// C `svt_aom_get_disallow_8x8_default` (`enc_mode_config.c:8196`). EXPORTED.
#[must_use]
pub fn get_disallow_8x8_default() -> bool {
    false
}

/// C `svt_aom_get_disallow_8x8_rtc` (`enc_mode_config.c:8200`). EXPORTED.
#[must_use]
pub fn get_disallow_8x8_rtc(enc_mode: i8, aligned_width: u16, aligned_height: u16) -> bool {
    if dimensions_require_8x8(aligned_width, aligned_height) {
        return false;
    }
    enc_mode > M9
}

/// C `svt_aom_get_disallow_8x8_allintra` (`enc_mode_config.c:8212`). EXPORTED.
#[must_use]
pub fn get_disallow_8x8_allintra() -> bool {
    false
}

// ---------------------------------------------------------------------------
// NSQ geometry / search
// ---------------------------------------------------------------------------

/// C `svt_aom_get_nsq_geom_level_default` (`enc_mode_config.c:8216`). EXPORTED.
#[must_use]
pub fn get_nsq_geom_level_default(enc_mode: i8, coeff_lvl: InputCoeffLvl) -> u8 {
    if enc_mode <= M0 {
        if coeff_lvl == InputCoeffLvl::High {
            2
        } else {
            1
        }
    } else if enc_mode <= M5 {
        if coeff_lvl == InputCoeffLvl::High {
            3
        } else {
            2
        }
    } else {
        3
    }
}

/// C `svt_aom_get_nsq_geom_level_rtc` (`enc_mode_config.c:8235`). EXPORTED.
#[must_use]
pub fn get_nsq_geom_level_rtc() -> u8 {
    3
}

/// C `svt_aom_get_nsq_geom_level_allintra` (`enc_mode_config.c:8239`). EXPORTED.
#[must_use]
pub fn get_nsq_geom_level_allintra(enc_mode: i8) -> u8 {
    if enc_mode <= MR {
        1
    } else if enc_mode <= M3 {
        2
    } else if enc_mode <= M6 {
        3
    } else {
        0
    }
}

/// C `NSQ_MODULATION_MIN_LEVEL` (`enc_mode_config.c:8276`).
const NSQ_MODULATION_MIN_LEVEL: i32 = 8;

/// C `MAX_TEMPORAL_LAYERS` — the r0 threshold table's length.
const MAX_TEMPORAL_LAYERS: usize = 6;

/// C `svt_aom_get_nsq_search_level_default` (`enc_mode_config.c:8254`).
/// EXPORTED.
///
/// The C signature takes `PictureControlSet*`; the fields it reads are
/// `ppcs->temporal_layer_index`, `ppcs->r0_gen`, `ppcs->r0`, `slice_type`,
/// `temporal_layer_index` and `scs->seq_qp_mod`, spelled out here.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn get_nsq_search_level_default(
    enc_mode: i8,
    coeff_lvl: InputCoeffLvl,
    qp: u32,
    ppcs_temporal_layer_index: u8,
    r0_gen: bool,
    r0: f64,
    is_islice: bool,
    temporal_layer_index: u8,
    seq_qp_mod: u8,
) -> u8 {
    let mut nsq_search_level: i32 = if enc_mode <= M0 {
        let is_base = ppcs_temporal_layer_index == 0;
        if is_base { 2 } else { 3 }
    } else if enc_mode <= M2 {
        7
    } else if enc_mode <= M3 {
        9
    } else if enc_mode <= M4 {
        12
    } else if enc_mode <= M6 {
        15
    } else if enc_mode <= M7 {
        18
    } else {
        19
    };

    if nsq_search_level == 0 {
        return 0;
    }

    if nsq_search_level > NSQ_MODULATION_MIN_LEVEL && r0_gen {
        let r0_tab: [f64; MAX_TEMPORAL_LAYERS] = [0.10, 0.15, 0.20, 0.25, 0.25, 0.25];
        let r0_th = if is_islice {
            0.05
        } else {
            r0_tab[temporal_layer_index as usize]
        };
        if r0 < r0_th {
            nsq_search_level =
                nsq_search_level.min(NSQ_MODULATION_MIN_LEVEL.max((nsq_search_level - 4).max(1)));
        }
    }

    // Offset by coeff_lvl.
    if coeff_lvl == InputCoeffLvl::High {
        nsq_search_level = if nsq_search_level + 2 > 19 {
            0
        } else {
            nsq_search_level + 2
        };
    } else if coeff_lvl == InputCoeffLvl::VLow || coeff_lvl == InputCoeffLvl::Low {
        nsq_search_level = (nsq_search_level - 3).max(1);
    }

    if nsq_search_level == 0 {
        return 0;
    }

    if seq_qp_mod != 0 {
        // NOTE the two arms differ only in the SECOND qp bound (45 vs 43) and
        // the fourth (59 vs 56) — transcribe both, do not fold them.
        if enc_mode <= M6 {
            if (seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 39 {
                nsq_search_level = if nsq_search_level + 3 > 19 {
                    0
                } else {
                    nsq_search_level + 3
                };
            } else if (seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 45 {
                nsq_search_level = if nsq_search_level + 2 > 19 {
                    0
                } else {
                    nsq_search_level + 2
                };
            } else if (seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 48 {
                nsq_search_level = if nsq_search_level + 1 > 19 {
                    0
                } else {
                    nsq_search_level + 1
                };
            } else if (seq_qp_mod == 1 || seq_qp_mod == 2) && qp > 59 {
                nsq_search_level = (nsq_search_level - 1).max(1);
            }
        } else if (seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 39 {
            nsq_search_level = if nsq_search_level + 3 > 19 {
                0
            } else {
                nsq_search_level + 3
            };
        } else if (seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 43 {
            nsq_search_level = if nsq_search_level + 2 > 19 {
                0
            } else {
                nsq_search_level + 2
            };
        } else if (seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 48 {
            nsq_search_level = if nsq_search_level + 1 > 19 {
                0
            } else {
                nsq_search_level + 1
            };
        } else if (seq_qp_mod == 1 || seq_qp_mod == 2) && qp > 56 {
            nsq_search_level = (nsq_search_level - 1).max(1);
        }
    }
    nsq_search_level as u8
}

/// C `svt_aom_get_nsq_search_level_rtc` (`enc_mode_config.c:8326`). EXPORTED.
#[must_use]
pub fn get_nsq_search_level_rtc(coeff_lvl: InputCoeffLvl, qp: u32, seq_qp_mod: u8) -> u8 {
    let mut nsq_search_level: i32 = 19;

    if coeff_lvl == InputCoeffLvl::High {
        nsq_search_level = if nsq_search_level + 2 > 19 {
            0
        } else {
            nsq_search_level + 2
        };
    } else if coeff_lvl == InputCoeffLvl::VLow || coeff_lvl == InputCoeffLvl::Low {
        nsq_search_level = (nsq_search_level - 3).max(1);
    }
    if nsq_search_level == 0 {
        return 0;
    }
    if seq_qp_mod != 0 {
        if (seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 39 {
            nsq_search_level = if nsq_search_level + 3 > 19 {
                0
            } else {
                nsq_search_level + 3
            };
        } else if (seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 43 {
            nsq_search_level = if nsq_search_level + 2 > 19 {
                0
            } else {
                nsq_search_level + 2
            };
        } else if (seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 48 {
            nsq_search_level = if nsq_search_level + 1 > 19 {
                0
            } else {
                nsq_search_level + 1
            };
        } else if (seq_qp_mod == 1 || seq_qp_mod == 2) && qp > 56 {
            nsq_search_level = (nsq_search_level - 1).max(1);
        }
    }
    nsq_search_level as u8
}

/// C `svt_aom_get_nsq_search_level_allintra` (`enc_mode_config.c:8363`).
/// EXPORTED.
#[must_use]
pub fn get_nsq_search_level_allintra(
    enc_mode: i8,
    qp: u32,
    coeff_lvl: InputCoeffLvl,
    seq_qp_mod: u8,
) -> u8 {
    let mut nsq_search_level: i32 = if enc_mode <= M0 {
        3
    } else if enc_mode <= M1 {
        10
    } else if enc_mode <= M2 {
        14
    } else if enc_mode <= M3 {
        16
    } else {
        0
    };

    if (coeff_lvl == InputCoeffLvl::VLow || coeff_lvl == InputCoeffLvl::Low) && enc_mode <= MR {
        nsq_search_level = (nsq_search_level - 3).max(1);
    }
    if nsq_search_level == 0 {
        return 0;
    }
    if seq_qp_mod != 0 {
        if enc_mode <= M5 {
            if (seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 39 {
                nsq_search_level = if nsq_search_level + 3 > 19 {
                    0
                } else {
                    nsq_search_level + 3
                };
            } else if (seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 45 {
                nsq_search_level = if nsq_search_level + 2 > 19 {
                    0
                } else {
                    nsq_search_level + 2
                };
            } else if (seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 48 {
                nsq_search_level = if nsq_search_level + 1 > 19 {
                    0
                } else {
                    nsq_search_level + 1
                };
            } else if (seq_qp_mod == 1 || seq_qp_mod == 2) && qp > 59 {
                nsq_search_level = (nsq_search_level - 1).max(1);
            }
        } else if (seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 39 {
            nsq_search_level = if nsq_search_level + 3 > 19 {
                0
            } else {
                nsq_search_level + 3
            };
        } else if (seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 43 {
            nsq_search_level = if nsq_search_level + 2 > 19 {
                0
            } else {
                nsq_search_level + 2
            };
        } else if (seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 48 {
            nsq_search_level = if nsq_search_level + 1 > 19 {
                0
            } else {
                nsq_search_level + 1
            };
        } else if (seq_qp_mod == 1 || seq_qp_mod == 2) && qp > 56 {
            nsq_search_level = (nsq_search_level - 1).max(1);
        }
    }
    nsq_search_level as u8
}

// ---------------------------------------------------------------------------
// NIC
// ---------------------------------------------------------------------------

/// C `svt_aom_get_nic_level_default` (`enc_mode_config.c:4451`). EXPORTED.
#[must_use]
pub fn get_nic_level_default(enc_mode: i8, is_base: bool) -> u8 {
    if enc_mode <= MR {
        if is_base { 1 } else { 2 }
    } else if enc_mode <= M0 {
        if is_base { 2 } else { 4 }
    } else if enc_mode <= M1 {
        if is_base { 4 } else { 5 }
    } else if enc_mode <= M3 {
        if is_base { 5 } else { 6 }
    } else if enc_mode <= M5 {
        7
    } else if enc_mode <= M6 {
        8
    } else if enc_mode <= M7 {
        if is_base { 8 } else { 9 }
    } else if enc_mode <= M8 {
        9
    } else if enc_mode <= M9 {
        if is_base { 9 } else { 11 }
    } else {
        11
    }
}

/// C `svt_aom_get_nic_level_rtc` (`enc_mode_config.c:4477`). EXPORTED.
#[must_use]
pub fn get_nic_level_rtc(enc_mode: i8) -> u8 {
    if enc_mode <= M8 {
        9
    } else if enc_mode <= M9 {
        10
    } else {
        11
    }
}

/// C `svt_aom_get_nic_level_allintra` (`enc_mode_config.c:4488`). EXPORTED.
#[must_use]
pub fn get_nic_level_allintra(enc_mode: i8) -> u8 {
    if enc_mode <= M0 {
        1
    } else if enc_mode <= M2 {
        3
    } else if enc_mode <= M4 {
        5
    } else if enc_mode <= M6 {
        6
    } else if enc_mode <= M7 {
        7
    } else {
        11
    }
}

// ---------------------------------------------------------------------------
// Intra mode levels
// ---------------------------------------------------------------------------

/// `(intra_level, dist_based_ang_intra_level)`.
pub type IntraModeLevels = (u32, u32);

/// C `svt_aom_get_intra_mode_levels_default` (`enc_mode_config.c:5307`).
/// EXPORTED.
#[must_use]
pub fn get_intra_mode_levels_default(
    enc_mode: i8,
    is_islice: bool,
    is_base: bool,
    transition_present: i32,
) -> IntraModeLevels {
    if enc_mode <= MR {
        (if is_base { 1 } else { 2 }, 0)
    } else if enc_mode <= M2 {
        (if is_base { 1 } else { 2 }, if is_base { 0 } else { 1 })
    } else if enc_mode <= M5 {
        (if is_base { 1 } else { 6 }, if is_islice { 0 } else { 2 })
    } else if enc_mode <= M7 {
        (if is_base { 2 } else { 6 }, if is_islice { 0 } else { 2 })
    } else if enc_mode <= M11 {
        (
            if is_islice || transition_present == 1 {
                4
            } else {
                6
            },
            if is_islice { 0 } else { 2 },
        )
    } else {
        (MAX_INTRA_LEVEL - 1, 0)
    }
}

/// C `svt_aom_get_intra_mode_levels_rtc` (`enc_mode_config.c:5336`). EXPORTED.
#[must_use]
pub fn get_intra_mode_levels_rtc(
    enc_mode: i8,
    is_islice: bool,
    transition_present: i32,
    use_flat_ipp: bool,
) -> IntraModeLevels {
    if (!use_flat_ipp && enc_mode <= M7) || (use_flat_ipp && enc_mode <= M9) {
        (
            if is_islice || transition_present == 1 {
                1
            } else {
                6
            },
            1,
        )
    } else if enc_mode <= M8 {
        (
            if is_islice || transition_present == 1 {
                4
            } else {
                6
            },
            1,
        )
    } else if enc_mode <= M10 {
        (
            if is_islice || transition_present == 1 {
                4
            } else {
                6
            },
            2,
        )
    } else {
        (MAX_INTRA_LEVEL - 1, 0)
    }
}

/// C `svt_aom_get_intra_mode_levels_allintra` (`enc_mode_config.c:5359`).
/// EXPORTED.
#[must_use]
pub fn get_intra_mode_levels_allintra(enc_mode: i8) -> IntraModeLevels {
    if enc_mode <= M4 {
        (1, 0)
    } else if enc_mode <= M5 {
        (2, 0)
    } else if enc_mode <= M6 {
        (6, 0)
    } else if enc_mode <= M8 {
        (7, 0)
    } else {
        (8, 0)
    }
}

// ---------------------------------------------------------------------------
// bypass_encdec / update_cdf / chroma
// ---------------------------------------------------------------------------

/// C `svt_aom_get_bypass_encdec_default` (`enc_mode_config.c:8418`). EXPORTED.
#[must_use]
pub fn get_bypass_encdec_default(enc_mode: i8, encoder_bit_depth: u8) -> u8 {
    if encoder_bit_depth == EB_EIGHT_BIT {
        u8::from(enc_mode > M2)
    } else {
        u8::from(enc_mode > M7)
    }
}

/// C `svt_aom_get_bypass_encdec_rtc` (`enc_mode_config.c:8437`). EXPORTED.
///
/// Byte-for-byte the same body as the `_default` arm in v4.2.0; kept separate
/// because upstream may diverge them.
#[must_use]
pub fn get_bypass_encdec_rtc(enc_mode: i8, encoder_bit_depth: u8) -> u8 {
    if encoder_bit_depth == EB_EIGHT_BIT {
        u8::from(enc_mode > M2)
    } else {
        u8::from(enc_mode > M7)
    }
}

/// C `svt_aom_get_bypass_encdec_allintra` (`enc_mode_config.c:8457`). EXPORTED.
#[must_use]
pub fn get_bypass_encdec_allintra(enc_mode: i8) -> u8 {
    u8::from(enc_mode > M3)
}

/// C `svt_aom_get_update_cdf_level_default` (`enc_mode_config.c:8507`).
/// EXPORTED.
#[must_use]
pub fn get_update_cdf_level_default(enc_mode: i8, is_islice: bool, is_base: bool) -> u8 {
    if enc_mode <= M0 {
        1
    } else if enc_mode <= M3 {
        if is_base { 1 } else { 2 }
    } else if enc_mode <= M8 {
        u8::from(is_islice)
    } else {
        0
    }
}

/// C `svt_aom_get_update_cdf_level_rtc` (`enc_mode_config.c:8520`). EXPORTED.
#[must_use]
pub fn get_update_cdf_level_rtc(enc_mode: i8, is_islice: bool) -> u8 {
    if enc_mode <= M8 {
        u8::from(is_islice)
    } else {
        0
    }
}

/// C `svt_aom_get_update_cdf_level_allintra` (`enc_mode_config.c:8529`).
/// EXPORTED.
#[must_use]
pub fn get_update_cdf_level_allintra(enc_mode: i8) -> u8 {
    if enc_mode <= M3 {
        1
    } else if enc_mode <= M6 {
        2
    } else {
        0
    }
}

/// C `svt_aom_get_chroma_level_default` (`enc_mode_config.c:8547`). EXPORTED.
#[must_use]
pub fn get_chroma_level_default(enc_mode: i8, is_islice: bool) -> u8 {
    if enc_mode <= MR {
        1
    } else if enc_mode <= M0 {
        if is_islice { 1 } else { 4 }
    } else if enc_mode <= M5 {
        4
    } else {
        5
    }
}

/// C `svt_aom_get_chroma_level_rtc` (`enc_mode_config.c:8562`). EXPORTED.
#[must_use]
pub fn get_chroma_level_rtc(enc_mode: i8) -> u8 {
    if enc_mode <= M10 { 4 } else { 5 }
}

/// C `svt_aom_get_chroma_level_allintra` (`enc_mode_config.c:8572`). EXPORTED.
#[must_use]
pub fn get_chroma_level_allintra(enc_mode: i8) -> u8 {
    if enc_mode <= M0 {
        1
    } else if enc_mode <= M1 {
        2
    } else if enc_mode <= M5 {
        4
    } else {
        5
    }
}

// ---------------------------------------------------------------------------
// Loop restoration levels
// ---------------------------------------------------------------------------

/// C `svt_aom_get_sg_filter_level_default` (`enc_mode_config.c:1402`).
/// static — tier 4 directly, but reachable at tier 1 through
/// `svt_aom_get_enable_sg_default`.
///
/// SCOPE: this returns **3** for `enc_mode <= ENC_M3`, so SGR *is* live in
/// video mode at presets 0..3. `rust/CLAUDE.md` envelope guard 5 ("SGR is dead
/// for M0..M13") is an ALLINTRA-only statement and does not transfer.
#[must_use]
pub fn get_sg_filter_level_default(enc_mode: i8, input_resolution: u8, fast_decode: u8) -> u8 {
    let mut sg_filter_lvl = if enc_mode <= MR {
        1
    } else if enc_mode <= M3 {
        3
    } else {
        0
    };
    if input_resolution >= ResolutionRange::R8k.as_u8()
        || (fast_decode != 0 && !(input_resolution <= ResolutionRange::R360p.as_u8()))
    {
        sg_filter_lvl = 0;
    }
    sg_filter_lvl
}

/// C `svt_aom_get_sg_filter_level_rtc` (`enc_mode_config.c:1420`). static.
#[must_use]
pub fn get_sg_filter_level_rtc(_input_resolution: u8, _fast_decode: u8) -> u8 {
    // C computes 0 then conditionally re-zeroes it.
    0
}

/// C `svt_aom_get_sg_filter_level_allintra` (`enc_mode_config.c:1431`). static.
#[must_use]
pub fn get_sg_filter_level_allintra(enc_mode: i8) -> u8 {
    u8::from(enc_mode <= MR)
}

/// C `svt_aom_get_enable_sg_default` (`enc_mode_config.c:2673`). EXPORTED.
#[must_use]
pub fn get_enable_sg_default(enc_mode: i8, input_resolution: u8, fast_decode: u8) -> u8 {
    u8::from(get_sg_filter_level_default(enc_mode, input_resolution, fast_decode) > 0)
}

/// C `svt_aom_get_enable_sg_rtc` (`enc_mode_config.c:2679`). EXPORTED.
#[must_use]
pub fn get_enable_sg_rtc(input_resolution: u8, fast_decode: u8) -> u8 {
    u8::from(get_sg_filter_level_rtc(input_resolution, fast_decode) > 0)
}

/// C `svt_aom_get_enable_sg_allintra` (`enc_mode_config.c:2685`). EXPORTED.
#[must_use]
pub fn get_enable_sg_allintra(enc_mode: i8) -> u8 {
    u8::from(get_sg_filter_level_allintra(enc_mode) > 0)
}

// ---------------------------------------------------------------------------
// Deblocking level
// ---------------------------------------------------------------------------

/// C `dlf_level_modulation` (`enc_mode_config.c:1442`). static — tier 4.
///
/// Applied on NON-BASE pictures only, so it is inert on a key frame.
#[must_use]
pub fn dlf_level_modulation(
    default_dlf_level: u8,
    modulation_mode: u8,
    ref_skip_percentage: u8,
) -> u8 {
    let mut dlf_level = default_dlf_level;

    if modulation_mode == 1 || modulation_mode == 2 {
        if ref_skip_percentage < 25 {
            dlf_level = if dlf_level == 0 {
                6
            } else if dlf_level > 5 {
                5.max(dlf_level - 2)
            } else {
                dlf_level
            };
        } else if ref_skip_percentage < 50 {
            dlf_level = if dlf_level == 0 {
                7
            } else if dlf_level > 5 {
                dlf_level - 1
            } else {
                dlf_level
            };
        }
    }

    if (modulation_mode == 2 || modulation_mode == 3) && dlf_level > 4 {
        if ref_skip_percentage > 95 {
            dlf_level = if dlf_level >= 6 { 0 } else { dlf_level + 2 };
        } else if ref_skip_percentage > 75 {
            dlf_level = if dlf_level == 7 { 0 } else { dlf_level + 1 };
        }
    }

    dlf_level
}

/// C `get_dlf_level_default` (`enc_mode_config.c:1466`). static — tier 4.
///
/// Differs from `get_dlf_level_allintra` at nearly every preset and feeds the
/// frame header's `loop_filter_level` directly.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn get_dlf_level_default(
    enc_mode: i8,
    is_not_last_layer: u8,
    fast_decode: u8,
    resolution: ResolutionRange,
    is_base: bool,
    coeff_lvl: InputCoeffLvl,
    ref_skip_percentage: u8,
) -> u8 {
    let mut dlf_level: u8;
    // 0: off, 1: only towards bd-rate, 2: both sides, 3: only towards speed.
    let mut modulation_mode: u8 = 0;

    if fast_decode <= 1 || resolution <= ResolutionRange::R360p {
        if enc_mode <= M0 {
            dlf_level = 1;
        } else if enc_mode <= M3 {
            dlf_level = 2;
        } else if enc_mode <= M6 {
            dlf_level = if is_not_last_layer != 0 { 3 } else { 6 };
        } else if enc_mode <= M7 {
            dlf_level = if is_not_last_layer != 0 { 3 } else { 6 };
            modulation_mode = 3;
        } else if enc_mode <= M9 {
            dlf_level = if is_not_last_layer != 0 { 6 } else { 0 };
            modulation_mode = 3;
        } else if enc_mode <= M11 {
            dlf_level = if coeff_lvl == InputCoeffLvl::High {
                if is_base { 6 } else { 0 }
            } else if is_base {
                6
            } else if is_not_last_layer != 0 {
                7
            } else {
                0
            };
            modulation_mode = 3;
        } else {
            dlf_level = 0;
            modulation_mode = 3;
        }
    } else if enc_mode <= M6 {
        dlf_level = 4;
    } else if enc_mode <= M7 {
        dlf_level = 6;
        modulation_mode = 3;
    } else if enc_mode <= M10 {
        dlf_level = if is_not_last_layer != 0 { 6 } else { 0 };
        modulation_mode = 3;
    } else {
        dlf_level = if is_not_last_layer != 0 { 7 } else { 0 };
        modulation_mode = 3;
    }

    if !is_base {
        dlf_level = dlf_level_modulation(dlf_level, modulation_mode, ref_skip_percentage);
    }
    dlf_level
}

/// C `get_dlf_level_rtc` (`enc_mode_config.c:1512`). static — tier 4.
#[must_use]
pub fn get_dlf_level_rtc(enc_mode: i8, is_base: bool, ref_skip_percentage: u8) -> u8 {
    let mut dlf_level: u8;
    let modulation_mode: u8;
    if enc_mode <= M7 {
        dlf_level = 3;
        modulation_mode = 1;
    } else if enc_mode <= M9 {
        dlf_level = 6;
        modulation_mode = 3;
    } else if enc_mode <= M10 {
        dlf_level = 7;
        modulation_mode = 3;
    } else {
        dlf_level = 0;
        modulation_mode = 3;
    }
    if !is_base {
        dlf_level = dlf_level_modulation(dlf_level, modulation_mode, ref_skip_percentage);
    }
    dlf_level
}

/// C `get_dlf_level_allintra` (`enc_mode_config.c:1535`). static — tier 4.
#[must_use]
pub fn get_dlf_level_allintra(enc_mode: i8, fast_decode: u8, resolution: ResolutionRange) -> u8 {
    if fast_decode <= 1 || resolution <= ResolutionRange::R360p {
        if enc_mode <= M3 {
            1
        } else if enc_mode <= M5 {
            2
        } else {
            5
        }
    } else if enc_mode <= M7 {
        0
    } else {
        5
    }
}

// ---------------------------------------------------------------------------
// Picture PD0 level
// ---------------------------------------------------------------------------

/// C `ldp0_lvl_offset` (`enc_mode_config.c:8597`) — the QP-band PD0 offsets.
const LDP0_LVL_OFFSET: [u8; 4] = [2, 2, 1, 0];

/// The QP band index `set_pic_pd0_lvl_default` derives from
/// `scs->static_config.qp` (`enc_mode_config.c:8603`).
#[must_use]
pub fn pd0_qp_band_idx(qp: u32) -> usize {
    if qp <= 27 {
        0
    } else if qp <= 39 {
        1
    } else if qp <= 43 {
        2
    } else {
        3
    }
}

/// C `set_pic_pd0_lvl_default` (`enc_mode_config.c:8592`). static — tier 4.
///
/// Returns the value C stores in `pcs->pic_pd0_lvl`.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn set_pic_pd0_lvl_default(
    enc_mode: i8,
    is_base: bool,
    is_islice: bool,
    transition_present: bool,
    coeff_lvl: InputCoeffLvl,
    input_resolution: ResolutionRange,
    qp: u32,
    seq_qp_mod: u8,
    super_block_size: u16,
) -> u8 {
    let qp_band_idx = pd0_qp_band_idx(qp);
    let base_or_trans = is_base || transition_present;
    let cap = |v: i32| -> u8 { i32::from(MAX_PD0_LVL).min(v) as u8 };

    let mut lvl: u8 = if enc_mode <= M2 {
        0
    } else if enc_mode <= M3 {
        1
    } else if enc_mode <= M7 {
        if input_resolution <= ResolutionRange::R360p {
            3
        } else if input_resolution <= ResolutionRange::R480p {
            if base_or_trans { 3 } else { 5 }
        } else if coeff_lvl == InputCoeffLvl::High {
            if base_or_trans { 7 } else { 8 }
        } else if coeff_lvl == InputCoeffLvl::Normal {
            if base_or_trans { 4 } else { 6 }
        } else if base_or_trans {
            3
        } else {
            5
        }
    } else if enc_mode <= M8 {
        if input_resolution <= ResolutionRange::R360p {
            let qp_offset = if seq_qp_mod <= 1 {
                0
            } else {
                i32::from(LDP0_LVL_OFFSET[qp_band_idx])
            };
            cap(3 + qp_offset)
        } else if input_resolution <= ResolutionRange::R480p {
            let qp_offset = if seq_qp_mod <= 1 {
                0
            } else {
                (i32::from(LDP0_LVL_OFFSET[qp_band_idx]) - 1).max(0)
            };
            if base_or_trans {
                cap(3 + qp_offset)
            } else {
                cap(5 + qp_offset)
            }
        } else {
            let qp_offset = if seq_qp_mod <= 1 {
                0
            } else {
                (i32::from(LDP0_LVL_OFFSET[qp_band_idx]) - 1).max(0)
            };
            if coeff_lvl == InputCoeffLvl::High {
                if base_or_trans {
                    cap(7 + qp_offset)
                } else {
                    cap(8 + qp_offset)
                }
            } else if coeff_lvl == InputCoeffLvl::Normal {
                if base_or_trans {
                    cap(5 + qp_offset)
                } else {
                    cap(7 + qp_offset)
                }
            } else if base_or_trans {
                cap(3 + qp_offset)
            } else {
                cap(5 + qp_offset)
            }
        }
    } else if enc_mode <= M10 {
        let qp_offset = if seq_qp_mod <= 1 {
            0
        } else {
            i32::from(LDP0_LVL_OFFSET[qp_band_idx])
        };
        if input_resolution <= ResolutionRange::R360p {
            if coeff_lvl == InputCoeffLvl::VLow || coeff_lvl == InputCoeffLvl::Low {
                if base_or_trans {
                    cap(3 + qp_offset)
                } else {
                    cap(5 + qp_offset)
                }
            } else if coeff_lvl == InputCoeffLvl::Normal {
                if base_or_trans {
                    cap(4 + qp_offset)
                } else {
                    cap(6 + qp_offset)
                }
            } else if base_or_trans {
                cap(5 + qp_offset)
            } else {
                cap(7 + qp_offset)
            }
        } else if coeff_lvl == InputCoeffLvl::High {
            if base_or_trans {
                cap(7 + qp_offset)
            } else {
                cap(8 + qp_offset)
            }
        } else if coeff_lvl == InputCoeffLvl::Normal {
            if base_or_trans {
                cap(5 + qp_offset)
            } else {
                cap(7 + qp_offset)
            }
        } else if base_or_trans {
            cap(3 + qp_offset)
        } else {
            cap(5 + qp_offset)
        }
    } else if input_resolution <= ResolutionRange::R360p {
        let qp_offset = if seq_qp_mod <= 1 {
            0
        } else {
            i32::from(LDP0_LVL_OFFSET[qp_band_idx])
        };
        if coeff_lvl == InputCoeffLvl::VLow || coeff_lvl == InputCoeffLvl::Low {
            if base_or_trans {
                cap(3 + qp_offset)
            } else {
                cap(5 + qp_offset)
            }
        } else if coeff_lvl == InputCoeffLvl::Normal {
            if base_or_trans {
                cap(4 + qp_offset)
            } else {
                cap(6 + qp_offset)
            }
        } else if base_or_trans {
            cap(5 + qp_offset)
        } else {
            cap(7 + qp_offset)
        }
    } else if coeff_lvl == InputCoeffLvl::High {
        7
    } else if is_islice || transition_present {
        6
    } else {
        7
    };

    // SB128 is conservatively capped to PD0_LVL_0.
    if super_block_size == 128 {
        lvl = 0;
    }
    lvl
}

/// C `set_pic_pd0_lvl_rtc` (`enc_mode_config.c:8711`). static — tier 4.
#[must_use]
pub fn set_pic_pd0_lvl_rtc(
    enc_mode: i8,
    is_base: bool,
    is_islice: bool,
    transition_present: bool,
    input_resolution: ResolutionRange,
    super_block_size: u16,
) -> u8 {
    let mut lvl = if enc_mode <= M7 {
        if input_resolution <= ResolutionRange::R360p {
            1
        } else if is_base {
            3
        } else {
            4
        }
    } else if enc_mode <= M8 {
        if input_resolution <= ResolutionRange::R360p {
            if is_base { 1 } else { 3 }
        } else if is_base {
            3
        } else {
            5
        }
    } else if enc_mode <= M9 {
        if is_base { 5 } else { 7 }
    } else if is_islice || transition_present {
        6
    } else {
        7
    };
    if super_block_size == 128 {
        lvl = 0;
    }
    lvl
}

/// C `set_pic_pd0_lvl_allintra` (`enc_mode_config.c:8743`). static — tier 4.
///
/// The port previously hardcoded this per preset instead.
#[must_use]
pub fn set_pic_pd0_lvl_allintra(enc_mode: i8, super_block_size: u16) -> u8 {
    let mut lvl = if enc_mode <= M1 {
        0
    } else if enc_mode <= M8 {
        1
    } else {
        7
    };
    if super_block_size == 128 {
        lvl = 0;
    }
    lvl
}

// ---------------------------------------------------------------------------
// Inter tool levels
// ---------------------------------------------------------------------------

/// C `get_inter_compound_level` (`enc_mode_config.c:8757`). EXPORTED.
///
/// Drives the sequence-header bits `enable_jnt_comp` / `enable_masked_compound`
/// (`svt_aom_sig_deriv_pre_analysis_scs`).
#[must_use]
pub fn get_inter_compound_level(enc_mode: i8) -> u8 {
    if enc_mode <= M0 {
        3
    } else if enc_mode <= M2 {
        4
    } else {
        0
    }
}

/// C `svt_aom_get_obmc_level` (`enc_mode_config.c:8815`). EXPORTED.
///
/// 0 above M8, so this decides whether OBMC exists at all for a cell — the
/// gate in front of the already-ported OBMC blend DSP.
#[must_use]
pub fn get_obmc_level(enc_mode: i8, qp: u32, seq_qp_mod: u8) -> u8 {
    let mut obmc_level: u8 = if enc_mode <= MR {
        1
    } else if enc_mode <= M1 {
        3
    } else if enc_mode <= M5 {
        5
    } else if enc_mode <= M8 {
        6
    } else {
        0
    };

    // QP-banding. NOTE the guard is `!(enc_mode <= ENC_M0)`, i.e. M0 and MR are
    // exempt.
    if !(enc_mode <= M0) && obmc_level != 0 && seq_qp_mod != 0 {
        if enc_mode <= M3 {
            if (seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 43 {
                obmc_level += 2;
            } else if (seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 53 {
                obmc_level += 1;
            } else if (seq_qp_mod == 1 || seq_qp_mod == 2) && qp > 60 {
                obmc_level = if obmc_level == 1 { 1 } else { obmc_level - 1 };
            }
        } else if (seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 43 {
            obmc_level += 2;
        } else if (seq_qp_mod == 2 || seq_qp_mod == 3) && qp <= 55 {
            obmc_level += 1;
        } else if (seq_qp_mod == 1 || seq_qp_mod == 2) && qp > 59 {
            obmc_level = if obmc_level == 1 { 1 } else { obmc_level - 1 };
        }
    }
    obmc_level
}

/// C `DEFAULT` — the "not overridden by static_config" sentinel.
pub const CONFIG_DEFAULT: i32 = -1;

/// C `svt_aom_set_mfmv_config` (`enc_mode_config.c:10134`). EXPORTED.
///
/// Returns the value C stores in `scs->mfmv_enabled` — the sequence-level
/// outer gate on `mfmv_controls` and on `av1_setup_motion_field`
/// (`md_config_process.c:932`).
#[must_use]
pub fn set_mfmv_config(enc_mode: i8, rtc_tune: bool, config_enable_mfmv: i32) -> u8 {
    if config_enable_mfmv == CONFIG_DEFAULT {
        if rtc_tune {
            0
        } else {
            u8::from(enc_mode <= M10)
        }
    } else {
        config_enable_mfmv as u8
    }
}

/// The result of C `svt_aom_sig_deriv_pre_analysis_pcs`
/// (`enc_mode_config.c:2750`). EXPORTED.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreAnalysisPcs {
    /// `pcs->enable_me_16x16`
    pub enable_me_16x16: u8,
    /// `pcs->enable_me_8x8`
    pub enable_me_8x8: u8,
    /// `pcs->enable_hme_flag`
    pub enable_hme_flag: u8,
    /// `pcs->enable_hme_level0_flag`
    pub enable_hme_level0_flag: u8,
    /// `pcs->enable_hme_level1_flag`
    pub enable_hme_level1_flag: u8,
    /// `pcs->enable_hme_level2_flag`
    pub enable_hme_level2_flag: u8,
    /// `pcs->tf_enable_hme_flag`
    pub tf_enable_hme_flag: u8,
    /// `pcs->tf_enable_hme_level0_flag`
    pub tf_enable_hme_level0_flag: u8,
    /// `pcs->tf_enable_hme_level1_flag`
    pub tf_enable_hme_level1_flag: u8,
    /// `pcs->tf_enable_hme_level2_flag`
    pub tf_enable_hme_level2_flag: u8,
}

/// C `svt_aom_sig_deriv_pre_analysis_pcs` (`enc_mode_config.c:2750`). EXPORTED.
///
/// The resolution is derived from `scs->max_input_luma_width *
/// max_input_luma_height`, i.e. the SEQUENCE maximum, not the current picture.
#[must_use]
pub fn sig_deriv_pre_analysis_pcs(
    enc_mode: i8,
    max_input_luma_width: u16,
    max_input_luma_height: u16,
    rtc: bool,
) -> PreAnalysisPcs {
    // C multiplies two `uint16_t` fields (`sequence_control_set.h:113-114`),
    // which promote to `int`; widening to u32 here matches for every product
    // that does not overflow `int`, i.e. every representable frame.
    let resolution = ResolutionRange::from_luma_area(
        u32::from(max_input_luma_width) * u32::from(max_input_luma_height),
    );
    let enable_me_16x16 = get_enable_me_16x16(enc_mode);
    let enable_me_8x8 = if enable_me_16x16 != 0 {
        get_enable_me_8x8(enc_mode, resolution, rtc)
    } else {
        0
    };
    PreAnalysisPcs {
        enable_me_16x16,
        enable_me_8x8,
        enable_hme_flag: 1,
        enable_hme_level0_flag: 1,
        enable_hme_level1_flag: 1,
        enable_hme_level2_flag: 1,
        tf_enable_hme_flag: 1,
        tf_enable_hme_level0_flag: 1,
        tf_enable_hme_level1_flag: 1,
        tf_enable_hme_level2_flag: 1,
    }
}

/// C `svt_aom_is_ref_same_size` (`enc_mode_config.c:2857`). EXPORTED.
///
/// Read by the LPD1 arms. The port's envelope has reference scaling and
/// super-res OFF, so `ppcs->is_not_scaled` is true and this short-circuits —
/// but the whole predicate is translated rather than assumed.
#[must_use]
pub fn is_ref_same_size(
    is_not_scaled: bool,
    is_b_slice: bool,
    ref_present: bool,
    ref_width: u16,
    ref_height: u16,
    frame_width: u16,
    frame_height: u16,
) -> bool {
    if is_not_scaled {
        return true;
    }
    if !is_b_slice {
        return false;
    }
    if !ref_present {
        return false;
    }
    ref_width == frame_width && ref_height == frame_height
}
