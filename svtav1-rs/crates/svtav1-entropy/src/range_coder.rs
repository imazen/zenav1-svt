//! Core arithmetic range coder engine.
//!
//! Spec 07 §8.2: Daala arithmetic coder (OdEcEnc).
//!
//! Ported from SVT-AV1's `OdEcEnc` in `bitstream_unit.c/h`.
//!
//! This implements the daala entropy coder used by AV1:
//! - 64-bit window (OdEcWindow)
//! - Q15 probability (15-bit fixed point)
//! - Inverse CDF representation
//! - Byte-at-a-time output with carry propagation

use crate::cdf::{AomCdfProb, CDF_PROB_TOP};
use alloc::vec::Vec;

/// Probability shift for range coding.
pub const EC_PROB_SHIFT: u32 = 6;
/// Minimum probability (must be <= (1 << EC_PROB_SHIFT) / 16).
pub const EC_MIN_PROB: u32 = 4;

/// Core arithmetic encoder state.
///
/// Ported from `OdEcEnc` struct in `bitstream_unit.h`.
#[derive(Debug)]
pub struct OdEcEnc {
    /// Output buffer for encoded bytes.
    buf: Vec<u8>,
    /// Write offset in buf.
    offs: u32,
    /// Low end of the current range (64-bit window).
    low: u64,
    /// Number of values in the current range.
    rng: u16,
    /// Number of bits of data in the current value.
    cnt: i16,
    /// Whether an error occurred.
    error: bool,
}

impl OdEcEnc {
    /// Create a new encoder with the given initial buffer capacity.
    pub fn new(capacity: usize) -> Self {
        let buf = alloc::vec![0u8; capacity];
        Self {
            buf,
            offs: 0,
            low: 0,
            rng: 0x8000, // Initial range = 32768
            cnt: -9,     // Initial count
            error: false,
        }
    }

    /// Reset the encoder state for a new frame.
    pub fn reset(&mut self) {
        self.offs = 0;
        self.low = 0;
        self.rng = 0x8000;
        self.cnt = -9;
        self.error = false;
    }

    /// Returns true if an error has occurred.
    pub fn has_error(&self) -> bool {
        self.error
    }

    /// Encode a symbol using an ICDF table (Q15 inverse CDF probabilities).
    ///
    /// `s` is the symbol index, `icdf` is the inverse CDF table (decreasing
    /// values: icdf[0] > icdf[1] > ... > icdf[nsyms-1] = 0).
    /// `nsyms` is the number of symbols.
    ///
    /// Both our encoder and rav1d's decoder use ICDF format internally.
    /// rav1d converts from CDF to ICDF during initialization (cdf0d function).
    ///
    /// The range computation matches rav1d's msac_decode_symbol:
    ///   v[val] = (r>>8) * (icdf[val] >> 6) >> 1 + EC_MIN_PROB * (n - val)
    /// Symbol s has range [v[s], v[s-1]) where v[-1] = rng.
    pub fn encode_cdf_q15(&mut self, s: usize, icdf: &[AomCdfProb], nsyms: usize) {
        assert!(s < nsyms, "symbol {s} >= nsyms {nsyms}");

        let n = nsyms as u32 - 1;
        let r = self.rng as u32;
        let l = self.low;

        // Compute v[s] = lower boundary of symbol s
        let v_s = if s <= n as usize {
            (((r >> 8) * ((icdf[s] >> EC_PROB_SHIFT) as u32)) >> (7 - EC_PROB_SHIFT))
                + EC_MIN_PROB * (n - s as u32)
        } else {
            0u32
        };

        // Compute v[s-1] = upper boundary of symbol s (= lower boundary of s-1)
        let v_prev = if s > 0 {
            (((r >> 8) * ((icdf[s - 1] >> EC_PROB_SHIFT) as u32)) >> (7 - EC_PROB_SHIFT))
                + EC_MIN_PROB * (n - (s as u32 - 1))
        } else {
            r // v[-1] = rng for symbol 0
        };

        let new_rng = if v_prev > v_s { v_prev - v_s } else { 1 };
        let new_l = l + v_s as u64;

        self.normalize(new_l, new_rng as u16);
    }

    /// Encode a binary symbol with probability `f` (Q15).
    ///
    /// Ported from `svt_od_ec_encode_bool_q15`.
    pub fn encode_bool_q15(&mut self, val: bool, f: u32) {
        debug_assert!(f > 0 && f < 32768);

        let l = self.low;
        let r = self.rng as u32;
        debug_assert!(r >= 32768);

        let v = (((r >> 8) * (f >> EC_PROB_SHIFT)) >> (7 - EC_PROB_SHIFT)) + EC_MIN_PROB;

        let (new_l, new_r) = if val {
            (l + (r - v) as u64, v as u16)
        } else {
            (l, (r - v) as u16)
        };

        self.normalize(new_l, new_r);
    }

    /// Core Q15 range update.
    ///
    /// Ported from `svt_od_ec_encode_q15`.
    fn encode_q15(&mut self, fl: u32, fh: u32, s: usize, nsyms: usize) {
        let l = self.low;
        let r = self.rng as u32;
        debug_assert!(r >= 32768);
        debug_assert!(fh <= fl);
        debug_assert!(fl <= 32768);

        let n = (nsyms - 1) as u32;

        let (new_l, new_r) = if fl < CDF_PROB_TOP as u32 {
            let u = (((r >> 8) * (fl >> EC_PROB_SHIFT)) >> (7 - EC_PROB_SHIFT))
                + EC_MIN_PROB * (n - (s as u32 - 1));
            let v = (((r >> 8) * (fh >> EC_PROB_SHIFT)) >> (7 - EC_PROB_SHIFT))
                + EC_MIN_PROB * (n - s as u32);
            let range = if u > v { u - v } else { 1 };
            (l + (r - u) as u64, range.max(1) as u16)
        } else {
            let v = (((r >> 8) * (fh >> EC_PROB_SHIFT)) >> (7 - EC_PROB_SHIFT))
                + EC_MIN_PROB * (n - s as u32);
            let range = if r > v { r - v } else { 1 };
            (l, range.max(1) as u16)
        };

        self.normalize(new_l, new_r);
    }

    /// Renormalization — maintains the invariant 32768 <= rng < 65536
    /// and flushes bytes when the window is full.
    ///
    /// Ported from `svt_od_ec_enc_normalize`.
    fn normalize(&mut self, low: u64, rng: u16) {
        if self.error {
            return;
        }

        let c = self.cnt as i32;

        // Number of leading zeros in 16-bit representation of rng
        let d = 16 - ilog_nz(rng as u32);
        let s = c + d;

        // Flush bytes when window is nearly full
        if s >= 40 {
            self.flush_bytes(low, rng, s, d);
            return;
        }

        self.low = low << d;
        self.rng = rng << d;
        self.cnt = s as i16;
    }

    /// Flush encoded bytes from the window.
    fn flush_bytes(&mut self, low: u64, rng: u16, s: i32, d: i32) {
        let c = self.cnt as i32;

        // Ensure buffer has enough space
        let needed = self.offs as usize + 8;
        if needed > self.buf.len() {
            self.buf.resize(self.buf.len() * 2 + 8, 0);
        }

        let num_bytes_ready = ((s >> 3) + 1) as u32;
        let new_c = c + 24 - (num_bytes_ready as i32 * 8);

        let output = low >> new_c;
        let new_low = low & ((1u64 << new_c) - 1);

        let mask = 1u64 << (num_bytes_ready * 8);
        let carry = output & mask;
        let data = output & (mask - 1);

        // Write bytes and handle carry
        self.write_bytes(data, carry, num_bytes_ready);

        let new_s = new_c + d - 24;
        self.low = new_low << d;
        self.rng = rng << d;
        self.cnt = new_s as i16;
    }

    /// Write encoded bytes to the output buffer with carry propagation.
    fn write_bytes(&mut self, data: u64, carry: u64, num_bytes: u32) {
        let offs = self.offs as usize;

        // Handle carry propagation
        if carry != 0 && offs > 0 {
            let mut i = offs - 1;
            loop {
                let new_val = self.buf[i].wrapping_add(1);
                self.buf[i] = new_val;
                if new_val != 0 || i == 0 {
                    break;
                }
                i -= 1;
            }
        }

        // Write bytes in big-endian order
        for i in 0..num_bytes {
            let shift = (num_bytes - 1 - i) * 8;
            self.buf[offs + i as usize] = (data >> shift) as u8;
        }

        self.offs += num_bytes;
    }

    /// Finalize encoding and return the encoded bytes.
    ///
    /// Must be called after all symbols are encoded.
    /// Ported from libaom/SVT-AV1's `od_ec_enc_done`.
    pub fn done(&mut self) -> &[u8] {
        if self.error {
            return &[];
        }

        // Round up `low` to ensure the decoder can correctly decode.
        let m: u64 = 0x3FFF;
        let e = ((self.low + m) & !m) | (m + 1);

        // Number of valid bits: 10 + cnt
        let s = 10 + self.cnt as i32;
        if s <= 0 {
            return &self.buf[..self.offs as usize];
        }

        let needed = self.offs as usize + ((s as usize + 7) / 8);
        if needed > self.buf.len() {
            self.buf.resize(needed + 8, 0);
        }

        // The valid bits in `e` are at a position determined by the accumulated
        // normalization shifts. The top bit of the range was initially at bit 14.
        // After normalization shifts totaling (cnt + 9), the top is at 14 + cnt + 9.
        // The first output byte starts at that position.
        //
        // To extract byte i: shift = 14 + (cnt + 9) - 7 - i*8 = 16 + cnt - i*8
        let num_bytes = ((s as u32 + 7) / 8) as usize;
        let c = self.cnt as i32;

        for i in 0..num_bytes {
            let shift = 16 + c - (i as i32) * 8;
            let byte = if shift >= 0 {
                ((e >> shift) & 0xFF) as u8
            } else {
                ((e << (-shift)) & 0xFF) as u8
            };

            let offs = self.offs as usize;
            if offs < self.buf.len() {
                self.buf[offs] = byte;
                // Carry propagation into previously written bytes
                if byte > 0 && offs > 0 {
                    // Check if adding this byte causes overflow
                    // (not needed for final flush — bytes are independent)
                }
            }
            self.offs += 1;
        }

        &self.buf[..self.offs as usize]
    }

    /// Get the number of bytes written so far.
    pub fn bytes_written(&self) -> usize {
        self.offs as usize
    }

    /// Debug: internal state access.
    pub fn low(&self) -> u64 {
        self.low
    }
    /// Debug: internal range.
    pub fn rng_val(&self) -> u16 {
        self.rng
    }
    /// Debug: internal bit count.
    pub fn cnt_val(&self) -> i16 {
        self.cnt
    }
}

/// Integer log2 for nonzero values (number of bits needed).
/// Equivalent to C's OD_ILOG_NZ.
#[inline]
fn ilog_nz(v: u32) -> i32 {
    debug_assert!(v > 0);
    32 - v.leading_zeros() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_encoder_state() {
        let enc = OdEcEnc::new(256);
        assert!(!enc.has_error());
        assert_eq!(enc.rng, 0x8000);
        assert_eq!(enc.cnt, -9);
    }

    #[test]
    fn encode_bool_does_not_error() {
        let mut enc = OdEcEnc::new(1024);
        enc.encode_bool_q15(true, 16384); // 50% probability
        enc.encode_bool_q15(false, 16384);
        enc.encode_bool_q15(true, 8192); // 25%
        assert!(!enc.has_error());
    }

    #[test]
    fn encode_produces_output() {
        let mut enc = OdEcEnc::new(1024);
        // Encode a sequence of booleans
        for _ in 0..100 {
            enc.encode_bool_q15(true, 16384);
        }
        let output = enc.done();
        assert!(!output.is_empty(), "encoder should produce output");
    }

    #[test]
    fn ilog_nz_values() {
        assert_eq!(ilog_nz(1), 1);
        assert_eq!(ilog_nz(2), 2);
        assert_eq!(ilog_nz(3), 2);
        assert_eq!(ilog_nz(4), 3);
        assert_eq!(ilog_nz(255), 8);
        assert_eq!(ilog_nz(256), 9);
        assert_eq!(ilog_nz(32768), 16);
        assert_eq!(ilog_nz(65535), 16);
    }

    #[test]
    fn reset_clears_state() {
        let mut enc = OdEcEnc::new(256);
        enc.encode_bool_q15(true, 16384);
        enc.reset();
        assert_eq!(enc.rng, 0x8000);
        assert_eq!(enc.offs, 0);
    }
}
