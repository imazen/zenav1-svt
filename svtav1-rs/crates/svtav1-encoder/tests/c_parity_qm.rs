//! [SVT_HDR_MODE] QM quantize differential: the Rust QM kernels
//! (`qm::quantize_b_qm` / `qm::quantize_fp_qm`) vs the REAL exported C
//! functions (`svt_aom_quantize_b_c` / `svt_av1_quantize_fp_qm_c`), fed the
//! TRANSCRIBED `wt/iwt_matrix_ref` slices from the Rust side — one test
//! validates the kernel math AND the table transcription AND the
//! offset/adjusted-size indexing together (a wrong table row or offset
//! shifts every weight and cannot pass).
//!
//! Sweep: all 19 tx sizes x QM levels {0, 4, 6, 8, 10, 14} x both plane
//! classes x qindexes across the range, randomized coefficients in the
//! i16 contract range (see c_parity_quant.rs COEFF_BOUND note).

use svtav1_cref as cref;
use svtav1_encoder::{qm, quant};
use svtav1_entropy::scan_tables;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn coeff(&mut self, bound: i32) -> i32 {
        let v = (self.next() % (2 * bound as u64 + 1)) as i64 - bound as i64;
        v as i32
    }
}

fn row_from_table(t: &quant::QuantTable) -> cref::QuantRow {
    let n = |v: [i32; 2]| -> [i16; 2] { [v[0] as i16, v[1] as i16] };
    cref::QuantRow::new(
        n(t.zbin),
        n(t.round),
        n(t.quant),
        n(t.quant_shift),
        n(t.round_fp),
        n(t.quant_fp),
        n(t.dequant),
    )
}

const QINDEXES: [u8; 8] = [0, 20, 60, 100, 128, 190, 230, 255];
const QM_LEVELS: [usize; 6] = [0, 4, 6, 8, 10, 14];

#[test]
fn qm_quantize_matches_c_with_transcribed_tables() {
    let mut rng = Rng(0x51ac_0111_2026_0716);
    let mut cells = 0usize;
    for c_tx in 0..19usize {
        // DCT_DCT scan; QM only applies to 2D transforms so this is the
        // representative scan class (weights are raster-indexed anyway).
        let scan = scan_tables::scan(c_tx, scan_tables::TX_TYPE_TO_SCAN_INDEX[0] as usize);
        let n = scan.len();
        let log_scale = quant::TX_SCALE_TAB[c_tx];
        for &level in &QM_LEVELS {
            for is_chroma in [false, true] {
                let (wt, iwt) = qm::qm_slices(level, is_chroma, c_tx).expect("level < 15");
                assert!(wt.len() >= n, "tx {c_tx}: slice {} < scan {n}", wt.len());
                for &qidx in &QINDEXES {
                    let t = quant::build_quant_table(qidx);
                    let row = row_from_table(&t);
                    for _ in 0..3 {
                        let coeffs: Vec<i32> =
                            (0..n).map(|_| rng.coeff(i16::MAX as i32)).collect();

                        let mut rq = vec![0i32; n];
                        let mut rdq = vec![0i32; n];
                        let re = qm::quantize_b_qm(
                            &coeffs, scan, &t, log_scale, wt, iwt, &mut rq, &mut rdq,
                        );
                        let mut cq = vec![0i32; n];
                        let mut cdq = vec![0i32; n];
                        let ce = cref::quantize_b_qm(
                            &coeffs, &row, scan, log_scale, wt, iwt, &mut cq, &mut cdq,
                        );
                        assert_eq!(
                            (re, &rq, &rdq),
                            (ce, &cq, &cdq),
                            "quantize_b_qm tx {c_tx} level {level} chroma {is_chroma} q {qidx}"
                        );

                        let mut rq = vec![0i32; n];
                        let mut rdq = vec![0i32; n];
                        let re = qm::quantize_fp_qm(
                            &coeffs, scan, &t, log_scale, wt, iwt, &mut rq, &mut rdq,
                        );
                        let mut cq = vec![0i32; n];
                        let mut cdq = vec![0i32; n];
                        let ce = cref::quantize_fp_qm(
                            &coeffs, &row, scan, log_scale, wt, iwt, &mut cq, &mut cdq,
                        );
                        assert_eq!(
                            (re, &rq, &rdq),
                            (ce, &cq, &cdq),
                            "quantize_fp_qm tx {c_tx} level {level} chroma {is_chroma} q {qidx}"
                        );
                        cells += 2;
                    }
                }
            }
        }
    }
    println!("qm quantize parity: {cells} cells");
}
