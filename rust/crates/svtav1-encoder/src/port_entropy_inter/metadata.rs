//! Metadata OBUs — `write_obu_metadata` (C
//! `Source/Lib/Codec/entropy_coding.c:3683`) and `svt_aom_write_metadata_av1`
//! (`:3809`), plus the `add_trailing_bits` (`:3673`) they share with the
//! sequence- and frame-header writers.
//!
//! This is the last group of `entropy_coding.c`'s OBU writers with no port at
//! all: the port emits sequence-header, temporal-delimiter, frame-header and
//! tile-group OBUs, but had no `OBU_METADATA` path, so an HDR CLL / MDCV /
//! ITU-T T.35 payload handed to the encoder had nowhere to go.
//!
//! # Two upstream shapes reproduced on purpose
//!
//! * **`metadata_type` is written as a plain 8-bit literal**, not as the
//!   leb128 the AV1 specification calls for (`:3689`). It agrees with leb128
//!   for every type below 128, which is every type the enum defines, so it is
//!   invisible in practice — but it is upstream's encoding and byte-identity
//!   means reproducing it. Recorded here rather than "corrected".
//! * **The payload is written TWICE.** C's phase 1 writes the metadata at
//!   `data + obu_header_size` purely to MEASURE its size, then phase 2
//!   overwrites that region with the leb128 length and re-writes the payload
//!   after it (`:3824-3838`). The port measures instead of writing, which is
//!   the same bytes with none of the scratch traffic; the doubled write is C's
//!   buffer arithmetic, not part of the format.
//!
//! # Evidence
//!
//! Tier 4 (`docs/WORKING-ON-THIS.md` §4): both C functions are `static` or
//! take a `Bitstream*` chained through the SVT output-buffer allocator, so no
//! shim can drive them. What IS tier 1 is everything they are built out of —
//! `svt_aom_wb_write_literal`, `svt_aom_wb_write_bit`,
//! `svt_aom_wb_is_byte_aligned`, `svt_aom_wb_bytes_written`,
//! `svt_aom_uleb_size_in_bytes` and `svt_aom_uleb_encode` are all gated in
//! `tests/c_parity_entropy_block.rs`. The composition on top is pinned by
//! hand-derived vectors traced against the C source.
//!
//! # Reachability
//!
//! Nothing calls this yet — the public encoder API takes no metadata array.
//! Per §7 a faithful translation with no caller stays translated.

use crate::entropy::obu::{BitWriter, ObuType, uleb_encode, write_obu_header};
use alloc::vec::Vec;

/// C `EbAv1MetadataType` (`Source/API/EbSvtAv1Metadata.h:26`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum MetadataType {
    /// C `EB_AV1_METADATA_TYPE_AOM_RESERVED_0`.
    Reserved0 = 0,
    /// C `EB_AV1_METADATA_TYPE_HDR_CLL`.
    HdrCll = 1,
    /// C `EB_AV1_METADATA_TYPE_HDR_MDCV`.
    HdrMdcv = 2,
    /// C `EB_AV1_METADATA_TYPE_SCALABILITY`.
    Scalability = 3,
    /// C `EB_AV1_METADATA_TYPE_ITUT_T35`.
    ItutT35 = 4,
    /// C `EB_AV1_METADATA_TYPE_TIMECODE`.
    Timecode = 5,
    /// C `EB_AV1_METADATA_TYPE_FRAME_SIZE`.
    FrameSize = 6,
}

/// C `SvtMetadataT` (`EbSvtAv1Metadata.h:37`).
///
/// C's `payload` may be `NULL`, which every caller then tests; a slice cannot
/// be null, so "no payload" is an EMPTY slice and
/// [`write_metadata_obus`] skips it exactly where C's `!payload` test does.
#[derive(Clone, Copy, Debug)]
pub struct Metadata<'a> {
    /// C `metadata->type`.
    pub ty: MetadataType,
    /// C `metadata->payload` / `metadata->sz`.
    pub payload: &'a [u8],
}

/// C `add_trailing_bits` (entropy_coding.c:3673).
///
/// On a byte-aligned buffer it writes a whole `0x80` byte; otherwise one `1`
/// bit, relying on the remaining bits already being zero — which the port's
/// [`BitWriter`] guarantees because it only ever ORs set bits into a
/// zero-initialised byte.
pub fn add_trailing_bits(wb: &mut BitWriter) {
    if wb.bit_len().is_multiple_of(8) {
        wb.write_bits(0x80, 8);
    } else {
        wb.write_bit(true);
    }
}

/// C `write_obu_metadata` (entropy_coding.c:3683) — one metadata OBU's
/// PAYLOAD (type byte, data bytes, trailing bits), without the OBU header or
/// the length field.
///
/// Returns an empty vector for an empty payload, which is C's
/// `if (!metadata || !metadata->payload) return 0;`.
pub fn write_obu_metadata(md: &Metadata<'_>) -> Vec<u8> {
    if md.payload.is_empty() {
        return Vec::new();
    }
    let mut wb = BitWriter::new();
    // See the module doc: 8 raw bits, not a leb128.
    wb.write_bits(md.ty as u32, 8);
    for &b in md.payload {
        wb.write_bits(u32::from(b), 8);
    }
    add_trailing_bits(&mut wb);
    wb.into_data()
}

/// C `svt_aom_write_metadata_av1` (entropy_coding.c:3809) — every metadata
/// OBU of ONE type, concatenated in array order.
///
/// C returns `EB_ErrorBadParameter` for a null array and for a leb128 that
/// re-encodes to a different length than it measured; the first is
/// `Err(MetadataError::NoMetadata)` here, and the second cannot happen
/// because the port encodes once instead of measuring and re-encoding.
///
/// Entries whose type does not match `ty` are skipped, so C's caller pattern
/// (one call per type) produces the same bytes.
pub fn write_metadata_obus(
    metadata: &[Metadata<'_>],
    ty: MetadataType,
) -> Result<Vec<u8>, MetadataError> {
    if metadata.is_empty() {
        return Err(MetadataError::NoMetadata);
    }
    let mut out = Vec::new();
    for md in metadata {
        if md.ty != ty || md.payload.is_empty() {
            continue;
        }
        let payload = write_obu_metadata(md);
        out.extend_from_slice(&write_obu_header(ObuType::Metadata, false));
        out.extend_from_slice(&uleb_encode(payload.len() as u32));
        out.extend_from_slice(&payload);
    }
    Ok(out)
}

/// The one failure C reports from `svt_aom_write_metadata_av1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetadataError {
    /// C `EB_ErrorBadParameter` for `!metadata || !metadata->metadata_array`.
    NoMetadata,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tier 4, traced against entropy_coding.c:3683-3697: type byte, payload
    /// bytes, then a whole `0x80` trailing byte because the buffer is always
    /// byte-aligned at that point.
    #[test]
    fn metadata_payload_is_type_data_and_a_full_trailing_byte() {
        let md = Metadata {
            ty: MetadataType::HdrCll,
            payload: &[0x12, 0x34, 0x56],
        };
        assert_eq!(
            write_obu_metadata(&md),
            alloc::vec![1, 0x12, 0x34, 0x56, 0x80]
        );
    }

    /// C's `!payload` early-out: no OBU at all, not an empty one.
    #[test]
    fn empty_payload_writes_nothing() {
        let md = Metadata {
            ty: MetadataType::ItutT35,
            payload: &[],
        };
        assert!(write_obu_metadata(&md).is_empty());
        let all = write_metadata_obus(&[md], MetadataType::ItutT35).unwrap();
        assert!(all.is_empty());
    }

    /// `add_trailing_bits` takes its OTHER arm when the buffer is not byte
    /// aligned: a single 1 bit, which then pads to the byte with zeros.
    #[test]
    fn trailing_bits_take_the_unaligned_arm() {
        let mut wb = BitWriter::new();
        wb.write_bits(0b101, 3);
        add_trailing_bits(&mut wb);
        assert_eq!(wb.bit_len(), 4);
        assert_eq!(wb.data(), &[0b1011_0000]);
    }

    /// The type filter is C's: only matching entries produce an OBU, and the
    /// array order is preserved.
    #[test]
    fn only_the_requested_type_is_emitted_in_array_order() {
        let a = Metadata {
            ty: MetadataType::HdrCll,
            payload: &[1],
        };
        let b = Metadata {
            ty: MetadataType::HdrMdcv,
            payload: &[2],
        };
        let c = Metadata {
            ty: MetadataType::HdrCll,
            payload: &[3],
        };
        let out = write_metadata_obus(&[a, b, c], MetadataType::HdrCll).unwrap();
        // Two OBUs: header(1) + len(1) + payload(3 = type,data,0x80) each.
        assert_eq!(out.len(), 2 * (1 + 1 + 3));
        // OBU type 5 (METADATA) in bits 3..6 of the header byte, size field 1.
        assert_eq!(out[0] >> 3 & 0xF, ObuType::Metadata as u8);
        assert_eq!(out[1], 3);
        assert_eq!(&out[2..5], &[1, 1, 0x80]);
        assert_eq!(&out[7..10], &[1, 3, 0x80]);
    }

    /// C's `EB_ErrorBadParameter` for a null array.
    #[test]
    fn no_metadata_is_an_error_not_an_empty_stream() {
        assert_eq!(
            write_metadata_obus(&[], MetadataType::HdrCll),
            Err(MetadataError::NoMetadata)
        );
    }
}
