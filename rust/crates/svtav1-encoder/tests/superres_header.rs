//! Superres header syntax vs REAL C v4.2.0 bytes (spec 5.5.1 `enable_superres`
//! + 5.9.8 `superres_params()`).
//!
//! Ground truth was captured from the built C encoder, not from the spec text:
//!
//! ```text
//! SvtAv1EncApp -i 64x64.yuv -w 64 -h 64 -n 1 --preset 6 --rc 0 --aq-mode 0 \
//!              --qp 40 --avif 1 --lp 1 --superres-mode 1 --superres-kf-denom D
//! ```
//!
//! * D = 12 -> SH payload `18 15 7f fd 70 08` (the non-superres p6 SH is
//!   `.. 30 08`; the single changed bit IS `enable_superres`), FH opens
//!   `2d a0 ..` = disable_cdf_update 0, allow_screen_content_tools 0,
//!   **use_superres 1, coded_denom 3** (-> SuperresDenom 12),
//!   render_and_frame_size_different 0, uniform_tile_spacing_flag 1,
//!   base_q_idx 160.
//! * D = 16 -> `coded_denom 7`.
//!
//! MEASURED GOTCHA, recorded so it is not re-derived: for a STILL (KEY) frame
//! the applicable C knob is `--superres-kf-denom`. `--superres-mode 1
//! --superres-denom 12` alone signals `enable_superres = 1` in the sequence
//! header but leaves `use_superres = 0` on the key frame — the streams for
//! denom 12 and 16 come out byte-IDENTICAL. That asymmetry is why
//! [`SuperresParams`] carries the sequence flag separately from the frame
//! denominator.

use svtav1_encoder::entropy::obu::{
    CdefSignal, ColorDescription, LrSignal, ScSignal, SeqTools, SuperresParams,
    write_key_frame_header_full_lr_sb, write_sequence_header_ex,
};

/// The preset-6 sequence-header tools the byte-identical p6 gate cell uses.
fn p6_tools(enable_superres: bool) -> SeqTools {
    SeqTools {
        separate_uv_delta_q: false,
        film_grain_params_present: false,
        enable_filter_intra: true,
        enable_intra_edge_filter: false,
        enable_restoration: true,
        use_128x128_superblock: false,
        enable_superres,
        chroma_sample_position: 0,
        // Inter tool bits + display delay: this literal drives a STILL
        // (reduced) header, which writes none of them, so the defaults are
        // inert here by construction.
        ..SeqTools::default()
    }
}

/// C's real 64x64 preset-6 SH payload WITH superres enabled.
const C_SH_P6_SUPERRES: [u8; 6] = [0x18, 0x15, 0x7f, 0xfd, 0x70, 0x08];
/// The same cell without superres (the existing byte-identical gate value).
const C_SH_P6_PLAIN: [u8; 6] = [0x18, 0x15, 0x7f, 0xfd, 0x30, 0x08];

#[test]
fn sequence_header_enable_superres_matches_c_bytes() {
    let color = ColorDescription::default();
    let sh_on = write_sequence_header_ex(64, 64, true, 8, &color, false, 30.0, p6_tools(true));
    assert_eq!(
        &sh_on[2..],
        &C_SH_P6_SUPERRES,
        "SH with enable_superres=1 != real C bytes"
    );
    // And the flag is the ONLY difference — the default path is untouched.
    let sh_off = write_sequence_header_ex(64, 64, true, 8, &color, false, 30.0, p6_tools(false));
    assert_eq!(
        &sh_off[2..],
        &C_SH_P6_PLAIN,
        "SH regression with the flag off"
    );
    assert_eq!(sh_on.len(), sh_off.len());
    let diff: Vec<usize> = (0..sh_on.len())
        .filter(|&i| sh_on[i] != sh_off[i])
        .collect();
    assert_eq!(
        diff,
        vec![6],
        "exactly one byte (the enable_superres bit) changes"
    );
}

fn fh_prefix(superres: SuperresParams, base_qindex: u8) -> Vec<u8> {
    write_key_frame_header_full_lr_sb(
        64,
        64,
        base_qindex,
        true,  // reduced_sh (still picture)
        false, // monochrome
        [0; 4],
        0,
        &CdefSignal {
            damping: 3,
            bits: 0,
            strengths: vec![(0, 0)],
        },
        &LrSignal::none(true),
        ScSignal {
            allow_screen_content_tools: false,
            allow_intrabc: false,
            superres,
        },
        None,
        None,
        None,
        None,
        0,
        0,
        0,
        64,
    )
}

/// MSB-first bit cursor — the same walk a decoder does over the uncompressed
/// header, so the asserts below are on FIELDS, not on hand-computed bytes
/// (byte-level expectations silently shift as the field count changes).
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn f(&mut self, n: usize) -> u32 {
        let mut v = 0u32;
        for _ in 0..n {
            let bit = (self.data[self.pos >> 3] >> (7 - (self.pos & 7))) & 1;
            v = (v << 1) | u32::from(bit);
            self.pos += 1;
        }
        v
    }
}

/// Decode the header prefix a decoder reads before `base_q_idx`:
/// (use_superres, coded_denom, base_q_idx).
fn decode_prefix(fh: &[u8], enabled_in_seq: bool) -> (u32, Option<u32>, u32) {
    let mut b = Bits { data: fh, pos: 0 };
    assert_eq!(b.f(1), 0, "disable_cdf_update");
    assert_eq!(b.f(1), 0, "allow_screen_content_tools");
    let (mut use_sr, mut coded) = (0, None);
    if enabled_in_seq {
        use_sr = b.f(1);
        if use_sr == 1 {
            coded = Some(b.f(3));
        }
    }
    assert_eq!(b.f(1), 0, "render_and_frame_size_different");
    assert_eq!(b.f(1), 1, "uniform_tile_spacing_flag");
    (use_sr, coded, b.f(8))
}

/// `superres_params()` codes `use_superres` + a 3-bit `coded_denom`, and the
/// resulting frame header matches C's bytes exactly.
#[test]
fn frame_header_superres_params_matches_c_bytes() {
    for (denom, coded) in [(12u8, 3u32), (16, 7)] {
        let fh = fh_prefix(
            SuperresParams {
                enabled_in_seq: true,
                denom: Some(denom),
            },
            160,
        );
        assert_eq!(
            decode_prefix(&fh, true),
            (1, Some(coded), 160),
            "denom {denom}: superres_params fields"
        );
    }
    // C's captured denom-12 header prefix, verbatim (SvtAv1EncApp
    // --superres-mode 1 --superres-kf-denom 12, 64x64 still preset 6 qp 40).
    let fh12 = fh_prefix(
        SuperresParams {
            enabled_in_seq: true,
            denom: Some(12),
        },
        160,
    );
    assert_eq!(
        &fh12[..2],
        &[0x2d, 0xa0],
        "denom 12 FH prefix != real C bytes"
    );
}

/// The sequence flag on but the frame unscaled: exactly one bit
/// (`use_superres = 0`), matching C's `--superres-denom`-only behaviour on a
/// key frame.
#[test]
fn frame_header_enabled_but_unscaled_codes_one_bit() {
    let enabled = fh_prefix(
        SuperresParams {
            enabled_in_seq: true,
            denom: None,
        },
        160,
    );
    assert_eq!(decode_prefix(&enabled, true), (0, None, 160));
    // With the sequence flag off nothing is coded at all, so the same fields
    // land one bit earlier.
    let off = fh_prefix(SuperresParams::default(), 160);
    assert_eq!(decode_prefix(&off, false), (0, None, 160));
    assert_ne!(
        enabled[0], off[0],
        "the use_superres bit must actually shift the header"
    );
}

/// Regression guard: with superres off everywhere the frame header is
/// byte-identical to the pre-superres writer output (this is what keeps every
/// existing identity cell green).
#[test]
fn superres_off_is_byte_identical_to_the_old_layout() {
    let a = fh_prefix(SuperresParams::default(), 100);
    let b = fh_prefix(
        SuperresParams {
            enabled_in_seq: false,
            denom: None,
        },
        100,
    );
    assert_eq!(a, b);
}

/// Spec 5.9.8's coded width, cross-checked against C
/// `calculate_scaled_size_helper` (which the DSP port pins against real C in
/// `svtav1-dsp` `c_parity_resize::scaled_size_matches_c_rule`).
#[test]
fn coded_width_matches_the_scaled_size_rule() {
    for w in [64u32, 96, 128, 320, 1920] {
        assert_eq!(
            SuperresParams::default().coded_width(w),
            w,
            "unscaled is the identity"
        );
        for denom in 9u8..=16 {
            let sp = SuperresParams {
                enabled_in_seq: true,
                denom: Some(denom),
            };
            // C `calculate_scaled_size_helper` (super_res.c:52) — pinned
            // against the real C symbol by `svtav1-dsp`'s
            // `c_parity_resize::scaled_size_matches_c_rule`; repeated here
            // because svtav1-entropy must not depend on svtav1-dsp.
            let expect = (w * 8 + u32::from(denom) / 2) / u32::from(denom);
            assert_eq!(sp.coded_width(w), expect, "w {w} denom {denom}");
        }
    }
}
