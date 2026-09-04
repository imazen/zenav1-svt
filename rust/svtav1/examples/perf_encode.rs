//! Port half of the G4 performance gate (`tools/perf_gate.sh`).
//!
//! The timing sibling of `examples/identity_run.rs`: it generates the SAME
//! deterministic 4:2:0 content, writes the SAME raw I420 `.yuv` the C driver
//! consumes (so both encoders see identical input — apples-to-apples), encodes
//! it through `EncodePipeline` at the proven byte-identical still-picture CQP
//! config, writes the `.obu`, and MEASURES the encode.
//!
//! What is timed: ONLY `encode_frame_420` on a FRESH pipeline — the per-frame
//! encode work. `EncodePipeline::new` (the port's one-time setup, C's analogue
//! of `svt_av1_enc_init`) is excluded from the clock, exactly as the C harness
//! (`tools/perf_c_encode`) excludes `svt_av1_enc_init`. Each timed sample is a
//! fresh-pipeline first-frame KEY encode, matching C's fresh-handle single-frame
//! encode.
//!
//! Warmup: `[warmup]` fresh-pipeline encodes run first (untimed) to warm the
//! allocator / OS page cache / branch predictors; only the final encode is
//! reported. Symmetric with the C harness's warmup cycles.
//!
//! Usage: perf_encode <content> <width> <height> <cli_qp 0..63> <preset> <out_prefix> [warmup=1]
//!   content: uniform (y=128) | gradient (identity campaign's gradient)
//!            | raw:<in.yuv> (real content: an I420 8-bit file of exactly
//!              <width>x<height>, as `identity_run crop:/file:` writes it)
//! Output (stdout, machine-readable, one line): "ENCODE_NS=<n> BYTES=<m>"
//!         everything else (notes) -> stderr, so the driver parses stdout clean.
//!
//! # INTER cells (`SVTAV1_FRAMES=N`, N > 1)
//!
//! Unset, or =1, leaves EVERY byte of the still path above untouched — the
//! whole multi-frame block is skipped, the `.yuv` layout is the same single
//! frame, and no existing `perf_gate.sh` cell moves. This is the same
//! opt-in shape `identity_run.rs` uses, and it reads the SAME env vars
//! (`SVTAV1_FRAMES`, `SVTAV1_FRAME_SHIFT`, `SVTAV1_INTRA_PERIOD`,
//! `SVTAV1_HIER_LEVELS`) so one set of variables drives the byte harness and
//! the timing harness identically. The motion model is `identity_run`'s: a
//! horizontal translation by `SVTAV1_FRAME_SHIFT` px/frame with edge
//! replication, generated here so both encoders still consume the ONE `.yuv`.
//!
//! WHAT IS TIMED, and why it is the WHOLE SEQUENCE. The port encodes frame
//! `f` only after frame `f-1` has produced the reference it predicts from, so
//! there is no fresh-pipeline "encode just the inter frame" to time. The C
//! side is worse: its API is pipelined, so a per-frame host clock around
//! `send_picture` measures queue admission, not encode. Both harnesses
//! therefore time `send all N frames -> drain`, and the INTER frame's own
//! cost is obtained by DIFFERENCING two measured cells (N=2 minus N=1) on
//! each side — a subtraction of two measurements, never a projection.
//! `FRAME_NS=` is additionally printed (port only) as a per-frame breakdown;
//! it has no C counterpart and must not be compared across encoders.

use std::time::Instant;
use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

fn gen_content(content: &str, w: usize, h: usize) -> Vec<u8> {
    let mut y = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            y[r * w + c] = match content {
                "uniform" => 128,
                // Spec'd by the identity campaign (matches identity_run.rs).
                "gradient" => (((r * 255) / h) as u8) ^ (((c * 3) & 0x3f) as u8),
                other => panic!("unknown content {other:?} (use uniform|gradient)"),
            };
        }
    }
    y
}

/// Append planes to `path`, creating it on the FIRST call of a run and
/// appending on every later one, through a `BufWriter` — so the harness never
/// materialises a whole-sequence copy of the input just to write it out. See
/// the call site for why that matters to every memory number this binary
/// produces.
fn write_yuv(path: &str, planes: &[&[u8]]) {
    use std::io::Write;
    let f = std::fs::File::create(path).expect("create .yuv");
    let mut w = std::io::BufWriter::with_capacity(1 << 16, f);
    for p in planes {
        w.write_all(p).expect("write .yuv");
    }
    w.flush().expect("flush .yuv");
}

/// [`write_yuv`] in append mode — the sequence arm streams one frame at a time.
fn append_yuv(path: &str, planes: &[&[u8]]) {
    use std::io::Write;
    let f = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("append .yuv");
    let mut w = std::io::BufWriter::with_capacity(1 << 16, f);
    for p in planes {
        w.write_all(p).expect("write .yuv");
    }
    w.flush().expect("flush .yuv");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 7 && args.len() != 8 {
        eprintln!(
            "usage: {} <content> <width> <height> <cli_qp 0..63> <preset> <out_prefix> [warmup=1]",
            args[0]
        );
        std::process::exit(2);
    }
    let content = args[1].as_str();
    let w: usize = args[2].parse().expect("width");
    let h: usize = args[3].parse().expect("height");
    let qp: u8 = args[4].parse().expect("cli_qp");
    let preset: u8 = args[5].parse().expect("preset");
    let prefix = &args[6];
    let warmup: usize = args.get(7).map(|s| s.parse().expect("warmup")).unwrap_or(1);

    assert!(
        w.is_multiple_of(2) && h.is_multiple_of(2),
        "perf harness uses even dims (floor==ceiling chroma)"
    );

    let (cw, ch) = (w / 2, h / 2);
    // `raw:<path>` — a real-content I420 8-bit .yuv (w*h luma + 2*(w/2)*(h/2)
    // chroma, the floor layout `identity_run`'s `crop:`/`file:` modes write),
    // so a callgrind/timing cell can run on a corpus image instead of the
    // synthetic ladder. Same contract as `identity_run`'s `raw:`; the file is
    // re-written to `<prefix>.yuv` below so the C driver reads the ONE stream
    // both encoders consume, exactly as for synthetic content. REAL chroma —
    // the synthetic modes are flat u=v=128, this is the only mode that is not.
    let (y, u, v) = if let Some(path) = content.strip_prefix("raw:") {
        let bytes = std::fs::read(path).expect("read raw yuv");
        let ysz = w * h;
        let csz = cw * ch;
        assert!(
            bytes.len() >= ysz + 2 * csz,
            "raw yuv {} too small: {} < {}",
            path,
            bytes.len(),
            ysz + 2 * csz
        );
        (
            bytes[..ysz].to_vec(),
            bytes[ysz..ysz + csz].to_vec(),
            bytes[ysz + csz..ysz + 2 * csz].to_vec(),
        )
    } else {
        (
            gen_content(content, w, h),
            vec![128u8; cw * ch],
            vec![128u8; cw * ch],
        )
    };

    let n_frames: usize = std::env::var("SVTAV1_FRAMES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    assert!(
        (1..=256).contains(&n_frames),
        "SVTAV1_FRAMES must be 1..256, got {n_frames}"
    );
    // SVTAV1_VIDEO=1 takes the video arm at ONE frame — a video-mode KEY frame
    // and nothing else. It is a CONTROL, and it is the C driver's `SVT_AVIF=0`
    // by another name (capture_c_trace.c documents why that one exists): a
    // 2-frame cell changes TWO variables at once, the still-vs-video signal
    // derivation and the presence of an inter frame, and without this arm the
    // cost of the first cannot be separated from the cost of the second.
    // MEASURED 2026-09-02 at 256x256 p8: the port's video-mode key frame costs
    // 10.64 ms where its still-mode key frame costs 2.95 ms, so that variable
    // is not small and attributing the whole gap to "the inter frame" would be
    // wrong by a factor of three.
    let video_single = std::env::var_os("SVTAV1_VIDEO").is_some();
    if n_frames > 1 || video_single {
        encode_sequence(
            content, y, u, v, w, h, cw, ch, qp, preset, prefix, warmup, n_frames,
        );
        return;
    }

    // Write the raw I420 8-bit .yuv the C driver (tools/perf_c_encode) reads —
    // the ONE byte stream both encoders consume, keeping the comparison honest.
    //
    // STREAMED, not concatenated. A `Vec` holding Y+U+V is one whole extra copy
    // of the frame that stays live for the rest of the process, and this binary
    // is what `tools/mem_gate.sh` and `tools/mem_peak.sh` MEASURE: at 4 MP that
    // copy was 6.29 MB of the reported peak on the still arm and 12.58 MB on
    // the 2-frame arm, against `perf_c_encode`, which holds exactly one copy of
    // the sequence. Measured, and named as the harness's own share of the peak,
    // in benchmarks/mem_massif_2026-09-03.meta §3.
    write_yuv(&format!("{prefix}.yuv"), &[&y, &u, &v]);

    // Fresh-pipeline encode at the proven byte-identical still-picture CQP
    // config (identity_run.rs / capture_c_trace.c): bd8, 4:2:0, tiles 0/0, SB
    // derived by C's own rule. `new(w,h,preset,rc, hierarchical_levels=0,
    // intra_period=1)` == allintra/still.
    let build = || {
        let rc = RcConfig {
            mode: RcMode::Cqp,
            qp,
            ..RcConfig::default()
        };
        EncodePipeline::new(w as u32, h as u32, preset, rc, 0, 1)
            .with_bit_depth(8)
            .with_tile_rows_log2(0)
            .with_tile_cols_log2(0)
            .with_sb_size(None)
            .with_chroma_420(true)
    };

    // Untimed warmup: fresh pipeline each time (frame_count=0 first-frame path).
    for _ in 0..warmup {
        let mut p = build();
        let _ = p.encode_frame_420(&y, &u, &v, w);
    }

    // Timed sample: fresh pipeline (setup untimed), time only encode_frame_420.
    let mut p = build();
    if p.sb128_fallback {
        // Loud on stderr: a fallback means the port coded a different SB
        // geometry than C, so this cell is NOT byte-comparable. The driver's
        // per-cell `cmp` catches it too, but flag it here as well.
        eprintln!(
            "perf_encode: SB128-FALLBACK at {w}x{h} preset {preset} — not byte-comparable to C"
        );
    }
    let t = Instant::now();
    let obu = p.encode_frame_420(&y, &u, &v, w);
    let ns = t.elapsed().as_nanos();

    std::fs::write(format!("{prefix}.obu"), &obu).expect("write .obu");
    // The ONLY stdout line — the driver greps it.
    println!("ENCODE_NS={ns} BYTES={}", obu.len());
}

/// Translate a plane right by `dx`, replicating the left edge — the identical
/// motion model `identity_run.rs` uses, so a timing cell and a byte cell at the
/// same `(content, size, qp, preset, frames, shift)` encode the same pixels.
fn translate(src: &[u8], pw: usize, ph: usize, dx: usize) -> Vec<u8> {
    let mut out = vec![0u8; pw * ph];
    for r in 0..ph {
        for c in 0..pw {
            out[r * pw + c] = src[r * pw + c.saturating_sub(dx)];
        }
    }
    out
}

/// The `SVTAV1_FRAMES > 1` arm. See the module docs for what is timed.
#[allow(clippy::too_many_arguments)]
fn encode_sequence(
    _content: &str,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
    w: usize,
    h: usize,
    cw: usize,
    ch: usize,
    qp: u8,
    preset: u8,
    prefix: &str,
    warmup: usize,
    n_frames: usize,
) {
    let shift_px: usize = std::env::var("SVTAV1_FRAME_SHIFT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);
    let intra_period: u32 = std::env::var("SVTAV1_INTRA_PERIOD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(64);
    let hier: u8 = std::env::var("SVTAV1_HIER_LEVELS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    // ONE copy of the sequence, and it is the one the encoder reads.
    //
    // This loop used to build a second whole-sequence `Vec` (`yuv`) purely to
    // hand `std::fs::write` a single slice, AND to keep the caller's `y/u/v` on
    // top of frame 0's copy of them — three copies live at once. At 2048x2048
    // that is 31.45 MB of the peak this binary reports on the 2-frame arm,
    // where `perf_c_encode` holds 12.58 MB (benchmarks/mem_massif_2026-09-03.meta
    // §3). Streaming each frame to the file as it is produced, and MOVING the
    // caller's planes into frame 0 instead of cloning them, leaves exactly the
    // `frames` vector — the same one copy C's driver holds.
    let path = format!("{prefix}.yuv");
    let mut frames: Vec<(Vec<u8>, Vec<u8>, Vec<u8>)> = Vec::with_capacity(n_frames);
    write_yuv(&path, &[&y, &u, &v]);
    frames.push((y, u, v)); // MOVED, not cloned — see the comment above.
    for f in 1..n_frames {
        let dx = shift_px * f;
        let fy = translate(&frames[0].0, w, h, dx);
        let fu = translate(&frames[0].1, cw, ch, dx / 2);
        let fv = translate(&frames[0].2, cw, ch, dx / 2);
        append_yuv(&path, &[&fy, &fu, &fv]);
        frames.push((fy, fu, fv));
    }

    let build = || {
        let rc = RcConfig {
            mode: RcMode::Cqp,
            qp,
            ..RcConfig::default()
        };
        EncodePipeline::new(w as u32, h as u32, preset, rc, hier, intra_period)
            .with_bit_depth(8)
            .with_tile_rows_log2(0)
            .with_tile_cols_log2(0)
            .with_sb_size(None)
            .with_chroma_420(true)
    };

    // Untimed warmup: whole sequences, fresh pipeline each time.
    for _ in 0..warmup {
        let mut p = build();
        for (fy, fu, fv) in &frames {
            if p.try_encode_frame_420(fy, fu, fv, w).is_err() {
                break;
            }
        }
    }

    let mut p = build();
    let mut per_frame_ns: Vec<u128> = Vec::with_capacity(n_frames);
    let mut all = Vec::new();
    let t = std::time::Instant::now();
    for (f, (fy, fu, fv)) in frames.iter().enumerate() {
        let t0 = std::time::Instant::now();
        match p.try_encode_frame_420(fy, fu, fv, w) {
            Ok(bytes) => {
                per_frame_ns.push(t0.elapsed().as_nanos());
                std::fs::write(format!("{prefix}.obu.f{f}"), &bytes).expect("write frame obu");
                all.extend_from_slice(&bytes);
            }
            Err(e) => {
                // A refusal is not a timing result. Say so on stderr, write
                // what encoded, and exit 3 — the same code `identity_run` uses,
                // so a driver cannot read a refusal as a fast encode.
                std::fs::write(format!("{prefix}.obu"), &all).expect("write .obu");
                eprintln!("perf_encode: REFUSED by the encoder at frame {f}: {e}");
                std::process::exit(3);
            }
        }
    }
    let ns = t.elapsed().as_nanos();
    std::fs::write(format!("{prefix}.obu"), &all).expect("write .obu");
    let per: Vec<String> = per_frame_ns
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    println!(
        "ENCODE_NS={ns} BYTES={} FRAMES={n_frames} FRAME_NS={}",
        all.len(),
        per.join(",")
    );
}
