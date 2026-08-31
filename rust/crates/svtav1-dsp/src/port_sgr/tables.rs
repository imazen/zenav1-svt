//! Constant tables for the self-guided (SGR) restoration port.
//!
//! All three are TRANSCRIBED FROM `Codec/restoration.c` and PINNED against the
//! real C arrays at evidence tier 1 (`tests/c_parity_sgr.rs`).

use super::SgrParamsType;

/// `svt_aom_eb_sgr_params` (restoration.c:31) — the 16 signalled `ep` presets.
///
/// `r[i] == 0` means "skip filter i"; exactly one of the two may be zero (both
/// zero would be equivalent to skipping SGR entirely, which C asserts against).
/// Entries 0..=9 run both filters, 10..=13 the r=1 filter only, 14..=15 the
/// r=2 (fast) filter only. The `s` value for a skipped filter is -1 and is
/// never read.
pub const SGR_PARAMS: [SgrParamsType; 16] = [
    SgrParamsType {
        r: [2, 1],
        s: [140, 3236],
    },
    SgrParamsType {
        r: [2, 1],
        s: [112, 2158],
    },
    SgrParamsType {
        r: [2, 1],
        s: [93, 1618],
    },
    SgrParamsType {
        r: [2, 1],
        s: [80, 1438],
    },
    SgrParamsType {
        r: [2, 1],
        s: [70, 1295],
    },
    SgrParamsType {
        r: [2, 1],
        s: [58, 1177],
    },
    SgrParamsType {
        r: [2, 1],
        s: [47, 1079],
    },
    SgrParamsType {
        r: [2, 1],
        s: [37, 996],
    },
    SgrParamsType {
        r: [2, 1],
        s: [30, 925],
    },
    SgrParamsType {
        r: [2, 1],
        s: [25, 863],
    },
    SgrParamsType {
        r: [0, 1],
        s: [-1, 2589],
    },
    SgrParamsType {
        r: [0, 1],
        s: [-1, 1618],
    },
    SgrParamsType {
        r: [0, 1],
        s: [-1, 1177],
    },
    SgrParamsType {
        r: [0, 1],
        s: [-1, 925],
    },
    SgrParamsType {
        r: [2, 0],
        s: [56, -1],
    },
    SgrParamsType {
        r: [2, 0],
        s: [22, -1],
    },
];

/// `svt_aom_eb_x_by_xplus1` (restoration.c) — `round(256 * x / (x + 1))` with
/// the deliberate special case `0 -> 1` (NOT 0). C's own comment explains why:
/// `A[k]` is a blend factor and a value of 0 with `r == 2` can push `B[k]`
/// just past `2^(8 + bit_depth)` through the rounding in
/// `one_by_x[24]`. Transcribing the 0 entry as 0 would be a silent
/// off-by-one-LSB pixel bug on flat content.
pub const X_BY_XPLUS1: [i32; 256] = [
    1, 128, 171, 192, 205, 213, 219, 224, 228, 230, 233, 235, 236, 238, 239, 240, 241, 242, 243,
    243, 244, 244, 245, 245, 246, 246, 247, 247, 247, 247, 248, 248, 248, 248, 249, 249, 249, 249,
    249, 250, 250, 250, 250, 250, 250, 250, 251, 251, 251, 251, 251, 251, 251, 251, 251, 251, 252,
    252, 252, 252, 252, 252, 252, 252, 252, 252, 252, 252, 252, 252, 252, 252, 252, 253, 253, 253,
    253, 253, 253, 253, 253, 253, 253, 253, 253, 253, 253, 253, 253, 253, 253, 253, 253, 253, 253,
    253, 253, 253, 253, 253, 253, 253, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254,
    254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254,
    254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254,
    254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 256,
];

/// `svt_aom_eb_one_by_x` (restoration.c) — `round(2^12 / n)` for
/// `n` in `1 ..= MAX_NELEM` (25 = the largest box, `(2*2+1)^2`).
pub const ONE_BY_X: [i32; 25] = [
    4096, 2048, 1365, 1024, 819, 683, 585, 512, 455, 410, 372, 341, 315, 293, 273, 256, 241, 228,
    216, 205, 195, 186, 178, 171, 164,
];
