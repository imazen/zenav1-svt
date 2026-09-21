//! Screen-content detection probe: run `sc_detect::derive_sc` on an I420
//! file's Y plane across presets and dump the class bits + tool gates.
//!
//! Usage: probe_sc <i420.yuv> <w> <h>
//! Prints per-preset: sc_class0..5, palette_level, intrabc_level,
//! allow_intrabc, allow_sct — the whole gating chain a screen frame must
//! survive before a single palette/IBC candidate can be injected.

use svtav1_encoder::sc_detect::{ScArm, derive_sc, is_screen_content_antialiasing_aware};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("i420 path");
    let w: usize = args.next().expect("w").parse().unwrap();
    let h: usize = args.next().expect("h").parse().unwrap();
    let y = std::fs::read(&path).expect("read yuv");
    let y = &y[..w * h];

    for fast in [false, true] {
        let c = is_screen_content_antialiasing_aware(y, w, w, h, fast);
        eprintln!(
            "fast_detection={fast}: sc0={} sc1={} sc2={} sc3={} sc4={} sc5={}",
            c.sc_class0 as u8,
            c.sc_class1 as u8,
            c.sc_class2 as u8,
            c.sc_class3 as u8,
            c.sc_class4 as u8,
            c.sc_class5 as u8
        );
    }
    for preset in -1..=13i8 {
        let d = derive_sc(ScArm::Allintra, preset, y, w, w, h);
        println!(
            "p{preset:>3}: sc5={} palette_level={} intrabc_level={} allow_ibc={} allow_sct={}",
            d.classes.sc_class5 as u8,
            d.palette_level,
            d.intrabc_level,
            d.allow_intrabc as u8,
            d.allow_screen_content_tools as u8
        );
    }
}
