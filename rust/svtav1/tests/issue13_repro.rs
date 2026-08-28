//! Issue #13 witness: the 10-bit reconstruction the encoder publishes must
//! carry the loop restoration it signalled.
//!
//! Before the fix, `recon10` fed the Wiener SEARCH (which picks the per-unit
//! taps on 10-bit data and signals them in the frame header) but only the u8
//! chain was handed to `apply_restoration_frame`. So on a 10-bit encode the
//! bitstream told every conforming decoder to apply Wiener while no 10-bit
//! plane inside the port ever received it — and nothing could observe that,
//! because no post-filter 10-bit recon was published at all.
//!
//! The oracle is the AV1 reference decoder: `EncodePipeline::last_recon10_final`
//! cropped to the true coded dims must equal what `aomdec` outputs for the same
//! stream, sample for sample. That is the same contract `recon_parity.sh` /
//! `alignment_gate.sh` pin for 8-bit, at 10-bit, from Rust.
//!
//! ANTI-VACUITY: the test asserts loop restoration actually signalled Wiener
//! on luma. If a future change switched LR off at this cell the comparison
//! would pass without testing the property this file exists for, so that
//! outcome is a FAILURE here.
//!
//! SKIPPING IS CALLER-CONTROLLED, never silent: the decoder is found via
//! `$AOMDEC` or `aomdec` on `PATH`; absent both the test FAILS with
//! instructions. `ZENAV1_SKIP_DECODER_TESTS=1` skips it deliberately, and
//! that decision is visible in the chain (workflow -> env -> test).

use std::path::PathBuf;
use std::process::Command;

use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

const RESTORE_WIENER: u8 = 1;

/// The identity harness's `gradient` luma (`identity_run.rs`) — the same
/// content `issue11_repro.rs` proved fires luma Wiener at this cell.
fn gradient_y(w: usize, h: usize) -> Vec<u8> {
    let mut y = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            y[r * w + c] = (((r * 255) / h) as u8) ^ (((c * 3) & 0x3f) as u8);
        }
    }
    y
}

fn textured_uv(cw: usize, ch: usize, phase: usize) -> Vec<u8> {
    let mut p = vec![0u8; cw * ch];
    for r in 0..ch {
        for c in 0..cw {
            let ramp = ((r * 5 + c * 3 + phase) % 96) as i32;
            let tex = if (r / 3 + c / 3) % 2 == 0 { 22 } else { -18 };
            p[r * cw + c] = (96 + ramp + tex).clamp(16, 240) as u8;
        }
    }
    p
}

fn widen(s: &[u8]) -> Vec<u16> {
    s.iter().map(|&x| u16::from(x) << 2).collect()
}

fn find_aomdec() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("AOMDEC") {
        return Some(PathBuf::from(p));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join("aomdec"))
        .find(|p| p.is_file())
}

/// Parse a y4m written by `aomdec -o <f>.y4m` for a 10-bit 4:2:0 stream: the
/// first FRAME's samples as little-endian u16, in Y|U|V order at the TRUE
/// dims (chroma CEILING), which is exactly the layout the decoder outputs.
fn y4m_first_frame_u16(bytes: &[u8], w: usize, h: usize) -> Vec<u16> {
    let hdr_end = bytes
        .iter()
        .position(|&b| b == b'\n')
        .expect("y4m header line");
    let hdr = std::str::from_utf8(&bytes[..hdr_end]).expect("y4m header utf8");
    assert!(
        hdr.contains("C420p10"),
        "expected a 10-bit 4:2:0 y4m from aomdec, header was: {hdr}"
    );
    let frame_tag = bytes[hdr_end..]
        .windows(5)
        .position(|win| win == b"FRAME")
        .expect("y4m FRAME tag")
        + hdr_end;
    let data_start = bytes[frame_tag..]
        .iter()
        .position(|&b| b == b'\n')
        .expect("FRAME line end")
        + frame_tag
        + 1;
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let need = w * h + 2 * cw * ch;
    let data = &bytes[data_start..];
    assert!(
        data.len() >= 2 * need,
        "y4m frame short: {} bytes < {}",
        data.len(),
        2 * need
    );
    data[..2 * need]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u16::from_le_bytes(*p))
        .collect()
}

/// `383x512` bd10 4:2:0 preset 6 qp 40: the cell `issue11_repro.rs` pinned as
/// firing luma Wiener on the 10-bit search. True dims are non-8-aligned on
/// purpose — the 10-bit canvas is stored at the ALIGNED stride (384) and the
/// apply must crop to the true extent exactly like the u8 one.
#[test]
fn issue13_bd10_final_recon_matches_aomdec_when_wiener_is_signalled() {
    if std::env::var_os("ZENAV1_SKIP_DECODER_TESTS").is_some() {
        eprintln!(
            "issue13_repro: SKIPPED by ZENAV1_SKIP_DECODER_TESTS — the 10-bit recon-vs-aomdec \
             check did NOT run in this invocation"
        );
        return;
    }
    let aomdec = find_aomdec().expect(
        "aomdec not found. Set AOMDEC=<path to aomdec> (or put it on PATH), or set \
         ZENAV1_SKIP_DECODER_TESTS=1 to skip this check DELIBERATELY. It is not skipped by \
         default because it is the only check that the 10-bit recon the encoder publishes \
         is what a decoder produces (issue #13).",
    );

    let (w, h) = (383usize, 512usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let y = widen(&gradient_y(w, h));
    let u = widen(&textured_uv(cw, ch, 0));
    let v = widen(&textured_uv(cw, ch, 41));

    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp: 40,
        ..RcConfig::default()
    };
    let mut p = EncodePipeline::new(w as u32, h as u32, 6, rc, 0, 1)
        .with_bit_depth(10)
        .with_tile_rows_log2(0)
        .with_tile_cols_log2(0)
        .with_sb_size(None)
        .with_chroma_420(true)
        .with_recon_output(true);
    let obu = p
        .try_encode_frame_420_hbd(&y, &u, &v, w)
        .expect("383x512 bd10 4:2:0 preset 6 must encode");
    assert!(!obu.is_empty());

    // Anti-vacuity: LR must have signalled Wiener on luma, otherwise the
    // apply under test never runs and this comparison proves nothing.
    assert_eq!(
        p.last_lr_stats.0[0], RESTORE_WIENER,
        "loop restoration did not fire on luma at this cell (frame types {:?}); pick \
         content/qp that does rather than letting the test pass vacuously",
        p.last_lr_stats.0
    );

    let (ry, ru, rv) = p.last_recon10_final.as_ref().expect(
        "with_recon_output(true) on a bd10 in-envelope frame must publish last_recon10_final",
    );
    // Crop the ALIGNED-stride canvas to the true dims (chroma CEILING), the
    // decoder's output layout.
    let aw = p.width as usize;
    let acw = aw / 2;
    let mut enc: Vec<u16> = Vec::with_capacity(w * h + 2 * cw * ch);
    for r in 0..h {
        enc.extend_from_slice(&ry[r * aw..r * aw + w]);
    }
    for pl in [ru, rv] {
        for r in 0..ch {
            enc.extend_from_slice(&pl[r * acw..r * acw + cw]);
        }
    }

    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("issue13");
    std::fs::create_dir_all(&dir).expect("create tmp dir");
    let obu_path = dir.join("bd10_383x512_p6_q40.obu");
    let y4m_path = dir.join("bd10_383x512_p6_q40.y4m");
    std::fs::write(&obu_path, &obu).expect("write obu");
    let status = Command::new(&aomdec)
        .arg("-o")
        .arg(&y4m_path)
        .arg(&obu_path)
        .status()
        .unwrap_or_else(|e| panic!("run {}: {e}", aomdec.display()));
    assert!(status.success(), "aomdec rejected the stream: {status}");
    let dec = y4m_first_frame_u16(&std::fs::read(&y4m_path).expect("read y4m"), w, h);

    assert_eq!(enc.len(), dec.len(), "sample count");
    let mismatches = enc.iter().zip(&dec).filter(|(a, b)| a != b).count();
    if mismatches != 0 {
        let i = enc.iter().zip(&dec).position(|(a, b)| a != b).unwrap();
        let (plane, pos) = if i < w * h {
            ("Y", format!("r{} c{}", i / w, i % w))
        } else {
            let j = (i - w * h) % (cw * ch);
            (
                if i < w * h + cw * ch { "U" } else { "V" },
                format!("r{} c{}", j / cw, j % cw),
            )
        };
        panic!(
            "10-bit final recon != aomdec: {mismatches} samples differ, first {plane}@{pos} \
             enc={} dec={} (issue #13 — the 10-bit canvas did not receive the signalled \
             loop restoration)",
            enc[i], dec[i]
        );
    }
}
