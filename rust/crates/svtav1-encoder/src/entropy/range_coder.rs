//! Core arithmetic range coder engine.
//!
//! Exact port of SVT-AV1's `OdEcEnc` from `bitstream_unit.c/h` — every
//! arithmetic operation mirrors the C reference so the emitted bytes are
//! bit-identical (verified by the differential tests in `tests/c_parity.rs`
//! against the linked C library).
//!
//! This implements the daala entropy coder used by AV1:
//! - 64-bit window (`OdEcWindow`), batched byte flushes
//! - Q15 probabilities in inverse-CDF representation (C layout: structural 0
//!   at `icdf[nsyms-1]`, adaptation counter — unused here — at `icdf[nsyms]`)
//! - Carry propagation both at flush time and at final `done()`

use crate::entropy::cdf::AomCdfProb;
use alloc::vec::Vec;

/// Probability shift for range coding (`EC_PROB_SHIFT`).
pub const EC_PROB_SHIFT: u32 = 6;
/// Minimum probability (`EC_MIN_PROB`, must be <= (1 << EC_PROB_SHIFT) / 16).
pub const EC_MIN_PROB: u32 = 4;

/// Core arithmetic encoder state.
///
/// Ported from the `OdEcEnc` struct in `bitstream_unit.h`.
#[derive(Debug)]
pub struct OdEcEnc {
    /// Output buffer for encoded bytes.
    ///
    /// Invariant: `buf.len() >= offs + 8` is (re-)established before every
    /// flush so the 8-byte batched store in `normalize` always fits, exactly
    /// like the C `storage` bookkeeping.
    buf: Vec<u8>,
    /// The offset at which the next entropy-coded byte will be written.
    offs: u32,
    /// The low end of the current range (64-bit window).
    low: u64,
    /// The number of values in the current range.
    rng: u16,
    /// The number of bits of data in the current value.
    cnt: i16,
    /// Whether an error occurred.
    error: bool,
}

impl OdEcEnc {
    /// Create a new encoder with the given initial buffer capacity.
    ///
    /// Ported from `svt_od_ec_enc_init` (growth happens on demand, so any
    /// capacity — including 0 — is valid).
    pub fn new(capacity: usize) -> Self {
        #[cfg(feature = "symtrace")]
        std::eprintln!("W RESET");
        Self {
            buf: alloc::vec![0u8; capacity],
            offs: 0,
            low: 0,
            rng: 0x8000,
            cnt: -9,
            error: false,
        }
    }

    /// Reset the encoder state for a new frame (`svt_od_ec_enc_reset`).
    pub fn reset(&mut self) {
        #[cfg(feature = "symtrace")]
        std::eprintln!("W RESET");
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

    /// Encode a symbol given a CDF table in Q15 (`svt_od_ec_encode_cdf_q15`).
    ///
    /// `icdf` is 32768 minus the CDF, such that symbol `s` falls in the range
    /// `[s > 0 ? (32768 - icdf[s-1]) : 0, 32768 - icdf[s])`. The values must
    /// be monotonically decreasing and `icdf[nsyms-1]` must be 0 (C layout;
    /// the adaptation counter lives one past it at `icdf[nsyms]`).
    pub fn encode_cdf_q15(&mut self, s: usize, icdf: &[AomCdfProb], nsyms: usize) {
        #[cfg(feature = "symtrace")]
        std::eprintln!(
            "W CDF nsyms={} s={} icdf=[{},{},{}] rng={}",
            nsyms,
            s,
            icdf[0],
            if nsyms >= 2 { icdf[1] } else { 0 },
            if nsyms >= 3 { icdf[2] } else { 0 },
            self.rng
        );
        debug_assert!(s < nsyms, "symbol {s} >= nsyms {nsyms}");
        debug_assert_eq!(icdf[nsyms - 1], 0, "C layout requires icdf[nsyms-1] == 0");
        // The `fh <= fl <= 32768` invariant the deleted `encode_q15` used to
        // assert, restated on the icdf the fused body reads directly.
        debug_assert!(
            u32::from(icdf[s]) <= if s > 0 { u32::from(icdf[s - 1]) } else { 32768 },
            "icdf must be monotonically decreasing"
        );
        // C's shape (bitstream_unit.c:284-296): `r_hi` and `temp` computed
        // ONCE and shared by both interval endpoints, and the `s > 0`
        // predicate tested ONCE. The port used to materialise
        // `fl = if s > 0 { icdf[s-1] } else { 32768 }` and hand it to
        // a private `encode_q15` helper (now deleted — C has no such function
        // any more either), which re-tested the same predicate as
        // `fl < 32768`, recomputed `r >> 8` in its `s == 0` arm and evaluated
        // `EC_MIN_PROB * (n - (s - 1))` and `EC_MIN_PROB * (n - s)`
        // separately. Byte-identical, arm by arm:
        //   s > 0: C's `u` = `((r_hi * (icdf[s-1] >> 6)) >> 1) + temp + 4`
        //          and the old `EC_MIN_PROB * (n - (s - 1))` is
        //          `4 * (nsyms - 1 - s) + 4` = `temp + EC_MIN_PROB`; C then
        //          sets `r = u` and subtracts the `fh` term, giving the old
        //          `r = u - v`.
        //   s == 0: the old `fl == 32768` arm is exactly C's fall-through,
        //          `r -= ((r_hi * (icdf[0] >> 6)) >> 1) + temp`.
        let mut l = self.low;
        let mut r = u32::from(self.rng);
        debug_assert!(r >= 32768);
        let r_hi = r >> 8;
        let temp = EC_MIN_PROB * (nsyms - 1 - s) as u32;
        if s > 0 {
            let u = ((r_hi * (u32::from(icdf[s - 1]) >> EC_PROB_SHIFT)) >> (7 - EC_PROB_SHIFT))
                + temp
                + EC_MIN_PROB;
            l += u64::from(r - u);
            r = u;
        }
        r -= ((r_hi * (u32::from(icdf[s]) >> EC_PROB_SHIFT)) >> (7 - EC_PROB_SHIFT)) + temp;
        self.normalize(l, r);
    }

    /// Encode a single binary value (`svt_od_ec_encode_bool_q15`).
    ///
    /// `f` is the probability that the value is one, scaled by 32768.
    pub fn encode_bool_q15(&mut self, val: bool, f: u32) {
        #[cfg(feature = "symtrace")]
        std::eprintln!("W BOOL val={} f={f} rng={}", u32::from(val), self.rng);
        debug_assert!(0 < f && f < 32768);
        let mut l = self.low;
        let mut r = u32::from(self.rng);
        debug_assert!(r >= 32768);
        let v = (((r >> 8) * (f >> EC_PROB_SHIFT)) >> (7 - EC_PROB_SHIFT)) + EC_MIN_PROB;
        if val {
            l += u64::from(r - v);
        }
        r = if val { v } else { r - v };
        self.normalize(l, r);
    }

    /// Renormalize so that `32768 <= rng < 65536`, flushing bytes from `low`
    /// to the output buffer when the window fills
    /// (`svt_od_ec_enc_normalize`, bitstream_unit.c:151-174).
    ///
    /// SHAPE, from C: this is the `static inline` hot path — compute `d`, test
    /// `c + d >= 40`, store three fields — and the flush is a separate
    /// `NOINLINE` function (C's `od_ec_enc_flush`, bitstream_unit.c:110, whose
    /// own comment says it is "kept out-of-line to reduce icache pressure on
    /// the hot (no-flush) path"). The port had ONE function carrying both
    /// paths, which LLVM then declined to inline anywhere: `normalize` was its
    /// own symbol costing 44.7 M Ir (2.78 % of the photo_cid p6 frame,
    /// `stall_attrib_2026-09-05` ranked the range coder #4 by cycles), so every
    /// symbol written paid a call, a prologue and a return around ~10
    /// instructions of work. Byte-inert: identical arithmetic, only the
    /// function boundary moved.
    #[inline]
    fn normalize(&mut self, low: u64, rng: u32) {
        let c = i32::from(self.cnt);
        debug_assert!(rng <= 65535);
        // The number of leading zeros in the 16-bit binary representation of rng.
        let d = 16 - ilog_nz(rng);
        // Flush whenever `low` cannot safely accommodate more data; see the C
        // source for the full derivation of the 40 == 56 - 16 threshold.
        // C spells this `EB_UNLIKELY(c + d >= 40)`.
        if c + d >= 40 {
            self.flush(low, rng, c, d);
        } else {
            self.low = low << d;
            self.rng = (rng << d) as u16;
            self.cnt = (c + d) as i16;
        }
    }

    /// [`Self::normalize`]'s cold arm — C `od_ec_enc_flush`
    /// (bitstream_unit.c:110), `NOINLINE` there and `#[inline(never)]` +
    /// `#[cold]` here for the same reason.
    #[inline(never)]
    #[cold]
    fn flush(&mut self, mut low: u64, rng: u32, mut c: i32, d: i32) {
        // `error` is never set anywhere in this file today, so this test is
        // dead — but it guards the only buffer WRITE in the coder, which is
        // where an error would have to be honoured, so it stays on the cold
        // side rather than being paid once per symbol on the hot one.
        if self.error {
            return;
        }
        let s = c + d;
        let offs = self.offs as usize;
        if offs + 8 > self.buf.len() {
            // C: storage = 2 * storage + 8 (values past `offs` are scratch).
            let new_len = 2 * self.buf.len() + 8;
            self.buf.resize(new_len, 0);
        }
        // One extra byte vs. s>>3 since cnt always counts one byte short
        // (it starts at -9).
        let num_bytes_ready = (s >> 3) + 1;
        // Number of non-ready bits left in `low` after extracting the
        // ready bytes (64-bit window: 24 == 64 - 40 cushion).
        c += 24 - (num_bytes_ready << 3);

        let output = low >> c;
        low &= (1u64 << c) - 1;

        let mask = 1u64 << (num_bytes_ready << 3);
        let carry = output & mask;

        // write_enc_data_to_out_buf: single big-endian 8-byte store with
        // the ready bytes left-aligned; bytes past `offs + num_bytes_ready`
        // are scratch and get overwritten by later flushes. The carry bit sits
        // at bit `num_bytes_ready * 8` and the shift is by
        // `64 - num_bytes_ready * 8`, so it lands at bit 64 and is discarded —
        // C says the same ("Carry bit will be shifted away, no need to mask")
        // and the port's extra `output &= mask - 1` was redundant.
        let reg = (output << ((8 - num_bytes_ready) << 3)).to_be_bytes();
        self.buf[offs..offs + 8].copy_from_slice(&reg);
        if carry != 0 {
            debug_assert!(self.offs > 0);
            propagate_carry_bwd(&mut self.buf, self.offs - 1);
        }
        self.offs += num_bytes_ready as u32;

        self.low = low << d;
        self.rng = (rng << d) as u16;
        self.cnt = (c + d - 24) as i16;
    }

    /// Finalize encoding and return the encoded bytes
    /// (`svt_od_ec_enc_done`).
    ///
    /// Call `reset` before reusing the encoder afterwards.
    pub fn done(&mut self) -> &[u8] {
        if self.error {
            return &[];
        }

        let l = self.low;
        let mut c = i32::from(self.cnt);
        // We output the minimum number of bits that ensures the symbols
        // encoded thus far decode correctly regardless of trailing bits.
        let mut s = 10 + c;
        let m: u64 = 0x3FFF;
        let mut e = ((l + m) & !m) | (m + 1);
        let mut offs = self.offs as usize;

        // Make sure there's enough room for the entropy-coded bits.
        let s_bits = (s + 7) >> 3;
        let b = s_bits.max(0) as usize;
        if offs + b > self.buf.len() {
            self.buf.resize(offs + b, 0);
        }

        if s > 0 {
            let mut n = (1u64 << (c + 16)) - 1;
            loop {
                let val = (e >> (c + 16)) as u16;
                self.buf[offs] = (val & 0x00FF) as u8;
                if val & 0x0100 != 0 {
                    debug_assert!(offs > 0);
                    propagate_carry_bwd(&mut self.buf, (offs - 1) as u32);
                }
                offs += 1;

                e &= n;
                s -= 8;
                c -= 8;
                n >>= 8;
                if s <= 0 {
                    break;
                }
            }
        }
        self.offs = offs as u32;
        #[cfg(feature = "symtrace")]
        std::eprintln!(
            "W DONE nbytes={} head={:02x?}",
            offs,
            &self.buf[..offs.min(16)]
        );
        &self.buf[..offs]
    }

    /// The number of bits "used" by the encoded symbols so far
    /// (`svt_od_ec_enc_tell`); always slightly larger than the exact value.
    pub fn tell(&self) -> i32 {
        // The 10 counteracts the -9 baked into cnt and reserves 1 bit for
        // terminating the stream.
        i32::from(self.cnt) + 10 + self.offs as i32 * 8
    }

    /// Get the number of bytes written so far.
    pub fn bytes_written(&self) -> usize {
        self.offs as usize
    }

    /// Debug: internal `low` window state.
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

/// Backward carry propagation (`propagate_carry_bwd`).
fn propagate_carry_bwd(buf: &mut [u8], offs: u32) {
    let mut offs = offs as usize;
    loop {
        let sum = u16::from(buf[offs]) + 1;
        buf[offs] = sum as u8;
        if sum >> 8 == 0 {
            break;
        }
        // A carry out of buf[0] would be a caller bug (the stream always
        // starts with a byte < 0xFF in valid use); underflow panics here.
        offs -= 1;
    }
}

/// Integer log2 for nonzero values (number of bits needed).
/// Equivalent to C's `OD_ILOG_NZ`.
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
        for _ in 0..100 {
            enc.encode_bool_q15(true, 16384);
        }
        let output = enc.done();
        assert!(!output.is_empty(), "encoder should produce output");
    }

    #[test]
    fn zero_capacity_grows() {
        let mut enc = OdEcEnc::new(0);
        let icdf = [16384u16, 0, 0];
        for i in 0..1000 {
            enc.encode_cdf_q15(i & 1, &icdf, 2);
        }
        let output = enc.done();
        assert!(
            output.len() >= 120,
            "~1 bit/symbol expected, got {}",
            output.len()
        );
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

    #[test]
    fn tell_advances() {
        let mut enc = OdEcEnc::new(256);
        let t0 = enc.tell();
        assert_eq!(t0, 1); // -9 + 10 + 0*8
        enc.encode_bool_q15(true, 16384);
        assert!(enc.tell() > t0);
    }
}
