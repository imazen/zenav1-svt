//! Chroma subsampling format — C `EbColorFormat`
//! (`EbSvtAv1Formats.h:110`: `EB_YUV400, EB_YUV420, EB_YUV422, EB_YUV444`)
//! and its `subsampling_x/y` derivation
//! (`Globals/enc_handle.c:4636-4637`,
//! `enc_dec_process.c:453-454`).
//!
//! Parity envelope, measured on v4.2.0: C's *library* parameterizes all
//! chroma geometry by `subsampling_x/y`, but `verify_settings`
//! (`Globals/enc_settings.c:470-474`) hard-refuses every
//! `encoder_color_format != EB_YUV420` with `SVT_ERROR("Only support 420
//! now")`. Consequence for the enum's tiers:
//!
//! * [`ChromaFormat::Yuv420`] is the ONLY C-parity surface — the byte
//!   oracle exists and every claim on it is identity-gated.
//! * `Yuv400`/`Yuv422`/`Yuv444` are Zen extensions with NO C byte oracle
//!   (C refuses them). Their oracle is the one the repo already uses for
//!   monochrome: conforming-decoder output (`aomdec`/`dav1d`) + the
//!   encoder's own final reconstruction, plus libaom (`zenav1-aom`)
//!   behavior for the syntax it signals.

/// C `EbColorFormat`, discriminants = C numbering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum ChromaFormat {
    /// 4:0:0 monochrome — `EB_YUV400`. Zen extension (C refuses);
    /// the port's existing mono path.
    Yuv400 = 0,
    /// 4:2:0 — `EB_YUV420`. The byte-parity surface.
    Yuv420 = 1,
    /// 4:2:2 — `EB_YUV422`. Zen extension (C refuses);
    /// subsamples horizontally only.
    Yuv422 = 2,
    /// 4:4:4 — `EB_YUV444`. Zen extension (C refuses);
    /// full-resolution chroma.
    Yuv444 = 3,
}

impl ChromaFormat {
    /// C `subsampling_x` (`enc_handle.c:4636`):
    /// `color_format == EB_YUV444 ? 0 : 1`.
    #[must_use]
    pub const fn subsampling_x(self) -> u8 {
        match self {
            Self::Yuv444 => 0,
            _ => 1,
        }
    }

    /// C `subsampling_y` (`enc_handle.c:4637`):
    /// `color_format >= EB_YUV422 ? 0 : 1`.
    #[must_use]
    pub const fn subsampling_y(self) -> u8 {
        match self {
            Self::Yuv422 | Self::Yuv444 => 0,
            _ => 1,
        }
    }

    /// Chroma plane width for a luma width — C's `(w + ss_x) >> ss_x`
    /// ceiling (`pcs.c` buffer descriptors, `(border + ss_x) >> ss_x`
    /// pattern). `Yuv400` reports 0 (no chroma planes exist).
    #[must_use]
    pub const fn chroma_width(self, luma_w: usize) -> usize {
        match self {
            Self::Yuv400 => 0,
            _ => luma_w.div_ceil(1usize << self.subsampling_x()),
        }
    }

    /// Chroma plane height — same ceiling on `subsampling_y`.
    #[must_use]
    pub const fn chroma_height(self, luma_h: usize) -> usize {
        match self {
            Self::Yuv400 => 0,
            _ => luma_h.div_ceil(1usize << self.subsampling_y()),
        }
    }

    /// AV1 `seq_profile` the format requires — spec §6.4.1 profile
    /// table, mirrored by C's `verify_settings` coupling
    /// (`enc_settings.c:475-490`) and `write_color_config`
    /// (`entropy_coding.c:2714-2745`): 444 -> profile 1 at 8/10-bit;
    /// EVERYTHING at 12-bit is profile 2 (profile 0/1 are 8/10-bit
    /// only, and profile 2 at <=10-bit is 4:2:2 only); 422 -> 2;
    /// 400/420 at 8/10-bit -> 0.
    #[must_use]
    pub const fn required_profile(self, bit_depth: u8) -> u8 {
        if bit_depth > 10 {
            return 2;
        }
        match self {
            Self::Yuv444 => 1,
            Self::Yuv422 => 2,
            _ => 0,
        }
    }

    /// `chroma_format_idc` written into the sequence header's
    /// color_config (`enc_handle.c:4636` context) — AV1 values, equal
    /// to the C `EbColorFormat` numbering for 400/420/422/444.
    #[must_use]
    pub const fn chroma_format_idc(self) -> u8 {
        self as u8
    }

    /// The chroma-plane block size for a luma `BlockSize` — C
    /// `get_plane_block_size` (`common_utils.h:135`) over
    /// `svt_aom_ss_size_lookup` (`common_utils.c:239`). `None` is C's
    /// `BLOCK_INVALID` (the subsampled size does not exist — e.g.
    /// BLOCK_4X8 at ss_x=1,ss_y=0).
    #[must_use]
    pub const fn plane_block_size(
        self,
        bsize: crate::block::BlockSize,
    ) -> Option<crate::block::BlockSize> {
        SS_SIZE_LOOKUP[bsize as usize][self.subsampling_x() as usize][self.subsampling_y() as usize]
    }

    /// C `is_chroma_reference` (`common_utils.h:315`): a luma block codes
    /// chroma when it is not strictly a sub-8-by-mi half of a subsampled
    /// pair. At 4:4:4 (`ss_x == ss_y == 0`) every block is a chroma
    /// reference — the `!ss_*` arms degenerate the rule to `true`.
    /// `mi_row`/`mi_col` are the block's 4x4-unit origin.
    #[must_use]
    pub const fn is_chroma_reference(
        self,
        mi_row: usize,
        mi_col: usize,
        bw_mi: usize,
        bh_mi: usize,
    ) -> bool {
        (mi_row % 2 == 1 || bh_mi.is_multiple_of(2) || self.subsampling_y() == 0)
            && (mi_col % 2 == 1 || bw_mi.is_multiple_of(2) || self.subsampling_x() == 0)
    }
}

/// C `svt_aom_ss_size_lookup` (`common_utils.c:239`), verbatim.
/// Indexed `[bsize][ss_x][ss_y]`; `None` is C's `BLOCK_INVALID`.
#[rustfmt::skip]
const SS_SIZE_LOOKUP: [[[Option<crate::block::BlockSize>; 2]; 2]; 22] = {
    use crate::block::BlockSize as B;
    [
        //  {ss0/ss0, ss0/ss1}  {ss1/ss0, ss1/ss1}
        [[Some(B::Block4x4),   Some(B::Block4x4)],   [Some(B::Block4x4),   Some(B::Block4x4)]],
        [[Some(B::Block4x8),   Some(B::Block4x4)],   [None,                Some(B::Block4x4)]],
        [[Some(B::Block8x4),   None],                [Some(B::Block4x4),   Some(B::Block4x4)]],
        [[Some(B::Block8x8),   Some(B::Block8x4)],   [Some(B::Block4x8),   Some(B::Block4x4)]],
        [[Some(B::Block8x16),  Some(B::Block8x8)],   [None,                Some(B::Block4x8)]],
        [[Some(B::Block16x8),  None],                [Some(B::Block8x8),   Some(B::Block8x4)]],
        [[Some(B::Block16x16), Some(B::Block16x8)],  [Some(B::Block8x16),  Some(B::Block8x8)]],
        [[Some(B::Block16x32), Some(B::Block16x16)], [None,                Some(B::Block8x16)]],
        [[Some(B::Block32x16), None],                [Some(B::Block16x16), Some(B::Block16x8)]],
        [[Some(B::Block32x32), Some(B::Block32x16)], [Some(B::Block16x32), Some(B::Block16x16)]],
        [[Some(B::Block32x64), Some(B::Block32x32)], [None,                Some(B::Block16x32)]],
        [[Some(B::Block64x32), None],                [Some(B::Block32x32), Some(B::Block32x16)]],
        [[Some(B::Block64x64), Some(B::Block64x32)], [Some(B::Block32x64), Some(B::Block32x32)]],
        [[Some(B::Block64x128),Some(B::Block64x64)], [None,                Some(B::Block32x64)]],
        [[Some(B::Block128x64),None],                [Some(B::Block64x64), Some(B::Block64x32)]],
        [[Some(B::Block128x128),Some(B::Block128x64)],[Some(B::Block64x128),Some(B::Block64x64)]],
        [[Some(B::Block4x16),  Some(B::Block4x8)],   [None,                Some(B::Block4x8)]],
        [[Some(B::Block16x4),  None],                [Some(B::Block8x4),   Some(B::Block8x4)]],
        [[Some(B::Block8x32),  Some(B::Block8x16)],  [None,                Some(B::Block4x16)]],
        [[Some(B::Block32x8),  None],                [Some(B::Block16x8),  Some(B::Block16x4)]],
        [[Some(B::Block16x64), Some(B::Block16x32)], [None,                Some(B::Block8x32)]],
        [[Some(B::Block64x16), None],                [Some(B::Block32x16), Some(B::Block32x8)]],
    ]
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsampling_matches_c_derive() {
        // enc_handle.c:4636-4637 verbatim truth table.
        for (f, sx, sy) in [
            (ChromaFormat::Yuv400, 1, 1),
            (ChromaFormat::Yuv420, 1, 1),
            (ChromaFormat::Yuv422, 1, 0),
            (ChromaFormat::Yuv444, 0, 0),
        ] {
            assert_eq!((f.subsampling_x(), f.subsampling_y()), (sx, sy), "{f:?}");
        }
    }

    #[test]
    fn chroma_dims() {
        assert_eq!(ChromaFormat::Yuv420.chroma_width(100), 50);
        assert_eq!(ChromaFormat::Yuv420.chroma_width(101), 51); // ceiling
        assert_eq!(ChromaFormat::Yuv422.chroma_height(100), 100); // full h
        assert_eq!(ChromaFormat::Yuv444.chroma_width(101), 101); // full w
        assert_eq!(ChromaFormat::Yuv400.chroma_width(64), 0);
    }

    #[test]
    fn profiles_per_c_verify_settings() {
        assert_eq!(ChromaFormat::Yuv444.required_profile(8), 1);
        assert_eq!(ChromaFormat::Yuv444.required_profile(10), 1); // p2@<=10b is 422-only
        assert_eq!(ChromaFormat::Yuv444.required_profile(12), 2);
        assert_eq!(ChromaFormat::Yuv422.required_profile(8), 2);
        assert_eq!(ChromaFormat::Yuv422.required_profile(12), 2);
        assert_eq!(ChromaFormat::Yuv420.required_profile(8), 0);
        assert_eq!(ChromaFormat::Yuv400.required_profile(8), 0);
        // 12-bit is always profile 2 — profiles 0/1 are 8/10-bit only.
        assert_eq!(ChromaFormat::Yuv420.required_profile(12), 2);
        assert_eq!(ChromaFormat::Yuv400.required_profile(12), 2);
    }

    #[test]
    fn ss_size_lookup_matches_c() {
        use crate::block::BlockSize as B;
        // Spot-checks across every shape class against common_utils.c:239.
        // 420 subsamples both axes.
        assert_eq!(
            ChromaFormat::Yuv420.plane_block_size(B::Block16x16),
            Some(B::Block8x8)
        );
        assert_eq!(
            ChromaFormat::Yuv420.plane_block_size(B::Block16x32),
            Some(B::Block8x16)
        );
        // 444 is the identity — every block is its own chroma block.
        for bs in B::ALL {
            assert_eq!(ChromaFormat::Yuv444.plane_block_size(bs), Some(bs));
        }
        // 422 subsamples only horizontally.
        assert_eq!(
            ChromaFormat::Yuv422.plane_block_size(B::Block16x16),
            Some(B::Block8x16)
        );
        // Vertically-tall blocks have no 4:2:2/4:2:0 chroma twin.
        assert_eq!(ChromaFormat::Yuv422.plane_block_size(B::Block8x16), None);
    }

    #[test]
    fn chroma_reference_rule() {
        // 444: every block is a chroma reference.
        for mi_r in 0..4 {
            for mi_c in 0..4 {
                for bw in [1usize, 2] {
                    for bh in [1usize, 2] {
                        assert!(ChromaFormat::Yuv444.is_chroma_reference(mi_r, mi_c, bw, bh));
                    }
                }
            }
        }
        // 420: only the bottom-right mi of a 2x2 pair of 1-mi blocks is
        // the chroma reference; even-sized blocks always cover the pair.
        assert!(!ChromaFormat::Yuv420.is_chroma_reference(0, 0, 1, 1));
        assert!(!ChromaFormat::Yuv420.is_chroma_reference(0, 1, 1, 1));
        assert!(!ChromaFormat::Yuv420.is_chroma_reference(1, 0, 1, 1));
        assert!(ChromaFormat::Yuv420.is_chroma_reference(1, 1, 1, 1));
        assert!(ChromaFormat::Yuv420.is_chroma_reference(0, 0, 2, 2));
    }
}
