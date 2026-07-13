//! Test-only FFI harness over the in-tree C SVT-AV1 static library.
//!
//! This crate exists solely for differential parity testing: every Rust
//! module ports a C module, and the tests here drive the *actual* C
//! implementation on identical inputs to assert bit-for-bit equality.
//!
//! This is the single sanctioned `unsafe` exception in the workspace — it is
//! `publish = false`, used only as a dev-dependency, and never part of a
//! shipped artifact.

use std::ffi::c_void;

unsafe extern "C" {
    fn ref_od_ec_enc_sizeof() -> usize;
    fn ref_od_ec_enc_alignof() -> usize;
    fn ref_od_ec_enc_init(enc: *mut c_void, size: u32);
    fn ref_od_ec_enc_reset(enc: *mut c_void);
    fn ref_od_ec_enc_clear(enc: *mut c_void);
    fn ref_od_ec_encode_cdf_q15(enc: *mut c_void, s: i32, icdf: *const u16, nsyms: i32);
    fn ref_od_ec_encode_bool_q15(enc: *mut c_void, val: i32, f: u32);
    fn ref_od_ec_enc_done(enc: *mut c_void, nbytes: *mut u32) -> *const u8;
    fn ref_od_ec_enc_error(enc: *const c_void) -> i32;
    fn ref_od_ec_enc_tell(enc: *const c_void) -> u32;
    fn ref_update_cdf(cdf: *mut u16, val: i8, nsymbs: i32);
    fn ref_write_symbol(enc: *mut c_void, symb: i32, cdf: *mut u16, nsymbs: i32);
}

/// The reference C range encoder (`OdEcEnc`), heap-allocated as an opaque blob.
pub struct RefEcEnc {
    /// Backing storage for the C struct; `u64` alignment covers the struct's
    /// requirement (verified against `_Alignof(OdEcEnc)` at construction).
    blob: Box<[u64]>,
    /// `done()` may only be called once before a reset.
    finished: bool,
}

impl RefEcEnc {
    /// Create and initialize a reference encoder with `size` bytes of initial
    /// buffer storage (the C side reallocs as needed).
    pub fn new(size: u32) -> Self {
        let bytes = unsafe { ref_od_ec_enc_sizeof() };
        let align = unsafe { ref_od_ec_enc_alignof() };
        assert!(align <= 8, "OdEcEnc alignment {align} exceeds u64 alignment");
        let words = bytes.div_ceil(8);
        let blob = vec![0u64; words].into_boxed_slice();
        let mut this = Self { blob, finished: false };
        unsafe { ref_od_ec_enc_init(this.ptr(), size) };
        this
    }

    fn ptr(&mut self) -> *mut c_void {
        self.blob.as_mut_ptr() as *mut c_void
    }

    fn cptr(&self) -> *const c_void {
        self.blob.as_ptr() as *const c_void
    }

    /// Encode symbol `s` with the given ICDF table (C layout: values then a
    /// structural 0 at `icdf[nsyms-1]`; slice must hold at least `nsyms`).
    pub fn encode_cdf_q15(&mut self, s: usize, icdf: &[u16], nsyms: usize) {
        assert!(!self.finished);
        assert!(s < nsyms && icdf.len() >= nsyms);
        unsafe { ref_od_ec_encode_cdf_q15(self.ptr(), s as i32, icdf.as_ptr(), nsyms as i32) };
    }

    /// Encode a boolean with probability `f` (Q15) that the value is one.
    pub fn encode_bool_q15(&mut self, val: bool, f: u32) {
        assert!(!self.finished);
        unsafe { ref_od_ec_encode_bool_q15(self.ptr(), i32::from(val), f) };
    }

    /// The real write path: encode symbol then adapt the CDF in place
    /// (C layout: slice must hold `nsymbs + 1` entries, counter at `[nsymbs]`).
    pub fn write_symbol(&mut self, symb: usize, cdf: &mut [u16], nsymbs: usize) {
        assert!(!self.finished);
        assert!(symb < nsymbs && cdf.len() >= nsymbs + 1);
        unsafe { ref_write_symbol(self.ptr(), symb as i32, cdf.as_mut_ptr(), nsymbs as i32) };
    }

    /// Bits "used" so far (reference `svt_od_ec_enc_tell`).
    pub fn tell(&self) -> u32 {
        unsafe { ref_od_ec_enc_tell(self.cptr()) }
    }

    /// Finalize and copy out the encoded bytes.
    pub fn done(&mut self) -> Vec<u8> {
        assert!(!self.finished, "done() called twice without reset");
        self.finished = true;
        let mut nbytes = 0u32;
        let p = unsafe { ref_od_ec_enc_done(self.ptr(), &mut nbytes) };
        assert!(!p.is_null(), "C encoder reported an error in done()");
        assert_eq!(unsafe { ref_od_ec_enc_error(self.cptr()) }, 0);
        unsafe { std::slice::from_raw_parts(p, nbytes as usize) }.to_vec()
    }

    /// Reset for reuse after `done()`.
    pub fn reset(&mut self) {
        unsafe { ref_od_ec_enc_reset(self.ptr()) };
        self.finished = false;
    }
}

impl Drop for RefEcEnc {
    fn drop(&mut self) {
        unsafe { ref_od_ec_enc_clear(self.ptr()) };
    }
}

/// Reference CDF adaptation (`update_cdf` from `cabac_context_model.h`).
/// C layout: `cdf[nsymbs]` is the adaptation counter, so the slice must hold
/// at least `nsymbs + 1` entries.
pub fn update_cdf(cdf: &mut [u16], val: usize, nsymbs: usize) {
    assert!(cdf.len() >= nsymbs + 1 && val < nsymbs);
    unsafe { ref_update_cdf(cdf.as_mut_ptr(), val as i8, nsymbs as i32) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_encode_and_done() {
        let mut enc = RefEcEnc::new(1024);
        // 2-symbol uniform ICDF, C layout: [16384, 0(structural), 0(counter)]
        let icdf = [16384u16, 0, 0];
        for i in 0..64 {
            enc.encode_cdf_q15(i & 1, &icdf, 2);
        }
        let bytes = enc.done();
        assert!(!bytes.is_empty(), "64 coin flips must produce output bytes");
        // ~1 bit/symbol -> ~8 bytes plus termination
        assert!(bytes.len() >= 8 && bytes.len() <= 12, "got {} bytes", bytes.len());
    }

    #[test]
    fn smoke_update_cdf_counter_position() {
        // C layout: counter at cdf[nsymbs] (index 4 for nsymbs=4).
        let mut cdf = [24576u16, 16384, 8192, 0, 0];
        update_cdf(&mut cdf, 2, 4);
        assert_eq!(cdf, [24832, 16896, 7936, 0, 1]);
    }

    #[test]
    fn smoke_write_symbol_adapts() {
        let mut enc = RefEcEnc::new(256);
        let mut cdf = [16384u16, 0, 0];
        enc.write_symbol(0, &mut cdf, 2);
        assert_eq!(cdf[2], 1, "counter must advance");
        assert_ne!(cdf[0], 16384, "probability must adapt");
        let _ = enc.done();
    }
}
