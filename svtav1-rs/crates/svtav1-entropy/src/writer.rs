//! High-level AV1 bitstream writer.
//!
//! Spec 07: High-level bitstream writer wrapping range coder.
//!
//! Wraps the range coder with CDF-based symbol writing and optional
//! backward-adaptive CDF updates.
//!
//! Ported from `AomWriter` in `bitstream_unit.h`.

use crate::cdf::{AomCdfProb, update_cdf};
use crate::range_coder::OdEcEnc;

/// AV1 bitstream writer.
///
/// Wraps the arithmetic coder engine with CDF management.
pub struct AomWriter {
    /// The core arithmetic encoder.
    pub ec: OdEcEnc,
    /// Whether to update CDFs after writing symbols.
    pub allow_update_cdf: bool,
}

impl AomWriter {
    /// Create a new writer with the given buffer capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            ec: OdEcEnc::new(capacity),
            allow_update_cdf: true,
        }
    }

    /// Write a single bit with the given probability (Q15).
    #[inline]
    pub fn write_bit(&mut self, bit: bool) {
        // 50/50 probability
        self.ec.encode_bool_q15(bit, 16384);
    }

    /// Write a bit with the given old-style probability (0-255 range).
    #[inline]
    pub fn write(&mut self, bit: bool, prob: u32) {
        let p = (0x7FFFFF - (prob << 15) + prob) >> 8;
        self.ec.encode_bool_q15(bit, p);
    }

    /// Write a literal value (fixed-width binary).
    #[inline]
    pub fn write_literal(&mut self, data: u32, bits: u32) {
        for bit in (0..bits).rev() {
            self.write_bit((data >> bit) & 1 != 0);
        }
    }

    /// Write a symbol using a CDF and optionally update the CDF.
    #[inline]
    pub fn write_symbol(&mut self, symb: usize, cdf: &mut [AomCdfProb], nsymbs: usize) {
        self.ec.encode_cdf_q15(symb, cdf, nsymbs);
        if self.allow_update_cdf {
            update_cdf(cdf, symb, nsymbs);
        }
    }

    /// Write a symbol using a CDF without updating it.
    #[inline]
    pub fn write_cdf(&mut self, symb: usize, cdf: &[AomCdfProb], nsymbs: usize) {
        self.ec.encode_cdf_q15(symb, cdf, nsymbs);
    }

    /// Finalize and return the encoded bitstream.
    pub fn done(&mut self) -> &[u8] {
        self.ec.done()
    }

    /// Get the number of bytes written so far.
    pub fn bytes_written(&self) -> usize {
        self.ec.bytes_written()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_literal() {
        let mut w = AomWriter::new(256);
        w.write_literal(0b1010, 4);
        let output = w.done();
        assert!(!output.is_empty());
    }

    #[test]
    fn writer_symbol() {
        let mut w = AomWriter::new(256);
        // 4-symbol CDF needs nsymbs+1 = 5 elements: [p0, p1, p2, sentinel, count]
        // p0 = prob(sym < 1), p1 = prob(sym < 2), p2 = prob(sym < 3)
        // In inverse CDF form: values decrease
        let mut cdf = [24576u16, 16384, 8192, 0, 0];
        w.write_symbol(0, &mut cdf, 4);
        w.write_symbol(1, &mut cdf, 4);
        w.write_symbol(2, &mut cdf, 4);
        w.write_symbol(3, &mut cdf, 4);
        let output = w.done();
        assert!(!output.is_empty());
    }

    #[test]
    fn writer_cdf_update() {
        let mut w = AomWriter::new(256);
        // Binary CDF needs nsymbs+1 = 3 elements: [p0, sentinel, count]
        let mut cdf = [16384u16, 0, 0];

        let initial = cdf[0];
        w.write_symbol(0, &mut cdf, 2);
        // After seeing symbol 0 in inverse CDF, cdf[0] should decrease
        assert!(cdf[0] < initial, "cdf[0] should decrease after val=0");
    }
}
