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
    assert!(
        bytes.len() >= 8 && bytes.len() <= 12,
        "got {} bytes",
        bytes.len()
    );
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
