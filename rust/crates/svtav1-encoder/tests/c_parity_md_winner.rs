//! Differential parity: full mode decision
//! (`svtav1-encoder/src/port_md_winner.rs`).
//!
//! **Evidence tier 1** for `svt_av1_is_lossless_segment` (mode_decision.c:71):
//! the oracle is the REAL exported symbol (`nm -g` prints `T`), driven over
//! every `(segmentation_enabled, segment_id)` pair against randomized
//! per-segment lossless flags. That sweep is what pins the detail worth
//! pinning — with segmentation OFF, C reads `lossless[0]` and IGNORES the
//! `segment_id` entirely.
//!
//! The winner selection itself is tier 4 and its vectors live beside the
//! code, with the reason stated in that module's doc.

use svtav1_cref::rd_cost as cref;
use svtav1_encoder::port_md_winner::is_lossless_segment;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
}

#[test]
fn is_lossless_segment_matches_c() {
    let segs = cref::max_segments();
    assert!(segs >= 1, "MAX_SEGMENTS must be positive");
    let mut r = Rng(0x1eaf_1e55);
    for _ in 0..2000 {
        let flags: Vec<i32> = (0..segs).map(|_| (r.next() & 1) as i32).collect();
        let bools: Vec<bool> = flags.iter().map(|&v| v != 0).collect();
        for enabled in [false, true] {
            for seg in 0..segs {
                let c = cref::is_lossless_segment(enabled, &flags, seg as i32);
                let p = is_lossless_segment(enabled, &bools, seg);
                assert_eq!(c, p, "enabled={enabled} seg={seg} flags={flags:?}");
            }
        }
    }
}
