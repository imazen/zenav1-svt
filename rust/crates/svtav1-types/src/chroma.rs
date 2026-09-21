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
}

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
}
