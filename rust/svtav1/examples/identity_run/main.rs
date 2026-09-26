//! Rust half of the bitstream-identity harness (tools/identity_diff.sh).
//!
//! Generates deterministic 4:2:0 content, writes it as a raw I420 .yuv
//! (the exact bytes the C driver `tools/capture_c_trace` consumes — both
//! encoders see identical input), encodes it through `EncodePipeline` in
//! 420 still-picture CQP mode, and writes the raw OBU stream.
//!
//! Build with `--features symtrace` and redirect stderr to capture the
//! per-symbol arithmetic-coder trace in the same format the wrapped C
//! library emits (`W CDF ...` / `W BOOL ...` lines).
//!
//! Usage: identity_run <content> <width> <height> <cli_qp 0..63> <preset> <out_prefix>
//!   content: uniform       — y = 128 everywhere
//!            gradient      — y[r][c] = ((r*255/h) ^ ((c*3) & 0x3f)), spec'd by
//!                            the identity campaign brief (trace_one's gradient)
//!            file:<a.png>  — decode the PNG, edge-replicate to <width>x<height>
//!                            if smaller, convert to I420 with the fixed
//!                            deterministic BT.601 limited-range transform below
//!                            (real photographic content — CID22, imazen26).
//!            crop:<a.png>  — like file: but CENTER-CROP a large image down to
//!                            <width>x<height> (used by the wider-corpus sweep to
//!                            run the big clic2025 / gb82-sc screen corpora at a
//!                            bounded encode size).
//!   u = v = 128 for uniform/gradient; real chroma for file: content.
//!
//! Writes <out_prefix>.yuv and <out_prefix>.obu. The critical harness
//! invariant is that this ONE .yuv is the exact byte stream the C driver
//! encodes too, so the RGB->YUV choice need not match any spec — only be
//! fixed and deterministic (both encoders see identical YUV).
//!//!
//! Environment: every variable is parsed once, strictly, in `cell.rs`
//! (`CellSpec`), with the C driver's name beside each. `SVTAV1_SB` has no C
//! twin on purpose: C derives the superblock size, and so does the port
//! unless the variable forces it (the anti-vacuity witness forces 64 on a
//! cell C codes at 128, which must then diverge).
//!
//! Exit status: 0 encoded, 3 the encoder refused, 4 the stop token
//! (`SVTAV1_TIMEOUT_MS`) fired.

mod cell;
mod content;
mod dump;

use cell::CellSpec;
use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 7 {
        eprintln!(
            "usage: {} <content> <width> <height> <cli_qp 0..63> <preset> <out_prefix>",
            args[0]
        );
        std::process::exit(2);
    }
    let content = args[1].as_str();
    let w: usize = args[2].parse().expect("width");
    let h: usize = args[3].parse().expect("height");
    let qp: u8 = args[4].parse().expect("cli_qp");
    let preset = svtav1_encoder::speed_config::NativePreset::new(
        args[5].parse::<i8>().expect("signed preset"),
    )
    .expect("native preset must be -1..=13");
    let prefix = &args[6];
    let spec = CellSpec::from_env();
    let timeout = spec.timeout;
    // I420 chroma dims: AV1 4:2:0 uses CEILING rounding for odd luma dims
    // ((w+1)/2), matching the port's `encode_frame_420` (which takes ceiling
    // chroma) and the pic-buffer/app convention. Task #95 goal 1: ODD true
    // dims (65x65, 65x64, 64x65) are now in scope for the synthetic content
    // paths (uniform/gradient/diag, flat u=v=128 chroma — the floor-vs-ceiling
    // choice is inert in flat chroma CONTENT; only the DLF chroma BOUND differs
    // at odd width, which the port replicates per the port-map). The file:/raw:
    // paths keep even dims (their 2x2 RGB averaging / floor .yuv layout needs
    // it). For EVEN dims ceiling == floor, so every existing cell is byte-
    // neutral. Chunk 1 (full-SB) + chunk 2 (partial-SB) already handled the
    // aligned/8-round + partial-SB edge coding; this only adds odd true dims.
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));

    let content::Source { y, u, v, rawseq } = content::load(content, w, h, spec.assert_nonflat);

    // SVTAV1_BD: encoder bit depth (8 default, or 10). At bd10 the C driver
    // (capture_c_trace <..> 10) reads PACKED u16 LE, so write the input as u16
    // (sample << (bd-8)); the port pipeline is u8 end-to-end, so it encodes the
    // u8 planes directly (chunks 2-4 add the u16 MD path). This is VALID for
    // content whose coded symbols are bit-depth-independent — uniform/skip,
    // where the decoder's DC prediction fills the 10-bit default and the coded
    // tile bytes are identical to bd8 apart from the SH high_bitdepth bit.
    let bd = spec.bd;
    // SVTAV1_HBD_SRC (task #6 chunk 2): generate a REAL 10-bit source instead
    // of `u8 << 2`. The low 2 bits carry a deterministic spatial pattern, so
    // both encoders see identical NON-widened 10-bit samples and the gate
    // actually exercises the u16 path end-to-end. The .yuv written below is
    // the SAME bytes the C driver reads, so the oracle needs no changes —
    // `capture_c_trace` already consumes a 16-bit-LE .yuv at bd10.
    let hbd_src = spec.hbd_src;
    // SVTAV1_HBD_PQ: a 10-bit source whose low bits come from a REAL transfer
    // curve rather than a synthetic pattern — the HDR shape of task #6.
    //
    // The luma is treated as sRGB-encoded, linearized, mapped onto a 1000-nit
    // display, run through the SMPTE ST 2084 (PQ) OETF, and quantized to
    // 10-bit LIMITED range (64..940). The chroma is rescaled from 8-bit
    // limited (16..240) to 10-bit limited (64..960) — a non-power-of-two
    // remap, so its low 2 bits also carry signal.
    //
    // WHAT THIS IS AND IS NOT, stated so nobody over-reads the gate that uses
    // it: it is a genuine 10-bit CODE-VALUE distribution — the low bits are a
    // consequence of a real nonlinear curve, and no `<< 2` can produce them —
    // derived from photographic content. It is NOT a native HDR camera
    // capture; the highlight detail an 8-bit master already clipped does not
    // come back. For what this gate tests — that the caller's low 2 bits
    // survive the u16 entry, the MD funnel, the coded levels and the
    // post-filter searches, byte-identically to C — the code-value
    // distribution is the load-bearing part.
    //
    // CICP is deliberately NOT varied. In MAINLINE v4.2.0 the encode is
    // CICP-independent apart from the header bits: the only reads of
    // `transfer_characteristics` / `color_primaries` in the codec are the
    // chroma-q boosts at rc_crf_cqp.c:573-586, which sit inside
    // `#if SVT_HDR_MODE`. `capture_c_trace` sets no CICP either, so both
    // encoders use the same defaults and the comparison stays about samples.
    let hbd_pq = spec.hbd_pq;
    let low_bits = |v: usize, r: usize, c: usize| -> u16 {
        // 0..3, varying in BOTH directions (a constant or row-only pattern
        // would be invisible to a horizontal-only predictor).
        (((r * 3 + c * 5 + v) % 4) as u16) & 3
    };
    // SMPTE ST 2084 PQ OETF on normalized linear light (1.0 = 10000 nits).
    let pq_oetf = |l: f64| -> f64 {
        const M1: f64 = 2610.0 / 16384.0;
        const M2: f64 = 2523.0 / 4096.0 * 128.0;
        const C1: f64 = 3424.0 / 4096.0;
        const C2: f64 = 2413.0 / 4096.0 * 32.0;
        const C3: f64 = 2392.0 / 4096.0 * 32.0;
        let lm = l.max(0.0).powf(M1);
        ((C1 + C2 * lm) / (1.0 + C3 * lm)).powf(M2)
    };
    let (y10, u10, v10): (Vec<u16>, Vec<u16>, Vec<u16>) = if hbd_pq {
        let y10 = y
            .iter()
            .map(|&s| {
                // sRGB EOTF on the 8-bit luma code, then a 1000-nit peak.
                let e = f64::from(s) / 255.0;
                let lin = if e <= 0.04045 {
                    e / 12.92
                } else {
                    ((e + 0.055) / 1.055).powf(2.4)
                };
                let n = pq_oetf(lin * 1000.0 / 10000.0);
                (64.0 + n * 876.0).round().clamp(0.0, 1023.0) as u16
            })
            .collect();
        // 8-bit limited (16..240) -> 10-bit limited (64..960).
        let cmap = |p: &[u8]| -> Vec<u16> {
            p.iter()
                .map(|&s| {
                    let v = (f64::from(s) - 16.0) * (960.0 - 64.0) / (240.0 - 16.0) + 64.0;
                    v.round().clamp(0.0, 1023.0) as u16
                })
                .collect()
        };
        (y10, cmap(&u), cmap(&v))
    } else if hbd_src {
        let shift = (bd - 8) as u32;
        let mk = |p: &[u8], pw: usize| -> Vec<u16> {
            p.iter()
                .enumerate()
                .map(|(i, &s)| ((s as u16) << shift) | low_bits(s as usize, i / pw, i % pw))
                .collect()
        };
        (mk(&y, w), mk(&u, cw), mk(&v, cw))
    } else {
        (Vec::new(), Vec::new(), Vec::new())
    };
    // INTER campaign chunk C0: SVTAV1_FRAMES=N encodes an N-frame sequence
    // instead of a single still, so the harness can produce the Rust half of an
    // INTER cell. Unset (or =1) leaves every byte of the still path below
    // untouched — this whole block is skipped and no existing cell moves.
    //
    // The motion model is a pure horizontal TRANSLATION of frame 0 by
    // `SVTAV1_FRAME_SHIFT` (default 3) pixels per frame, with edge
    // replication. It is applied to the already-generated planes rather than
    // by re-running content generation with an offset, so `file:`/`crop:`/
    // `raw:` content translates exactly like synthetic content does, and both
    // encoders still consume the ONE shared .yuv.
    //
    // Translation is deliberately the SIMPLEST real motion: a single global
    // integer-pel MV is the right answer for nearly every block, so the first
    // inter cell tests the plumbing (headers, DPB, MV coding) rather than the
    // search's ability to find a hard MV. Harder motion belongs in later cells.
    let n_frames = spec.frames;
    if n_frames > 1 {
        // 10-bit multi-frame is reached by SHIFTING each 8-bit frame up, the
        // same widening the single-frame bd10 path uses (`CellSpec` refuses
        // `SVTAV1_HBD_SRC` here: no multi-frame 10-bit source producer).
        let shift_px = spec.frame_shift.unwrap_or(3);
        // `SVTAV1_FRAME_ZOOM_NUM` / `_DEN` (default 1/1 — OFF) add a ZOOM about
        // the frame centre on top of the translation, so a cell can present a
        // motion field that is NOT a single global integer MV.
        //
        // WHY THIS EXISTS, MEASURED 2026-09-05. C gates its whole global-motion
        // search on `average_me_sad = sum(rc_me_distortion) / (w*h) >= 1`
        // (`global_me.c:157-172`). A pure translation is exactly what open-loop
        // ME finds, so the residual SAD is ~0 and `average_me_sad` floors to 0:
        // measured `avg_me_sad=0` and `is_gm_on=0` on ALL of
        // {gradient,diag,screen} x {64,128,256,512} AND on `crop:` photo
        // content at shifts 3/13/37 (`SVT_GM_OUT`, see
        // `docs/INTER-ENCODE-PLAN.md`). So the pre-existing grid cannot reach
        // C's GM search at all, and a GM gate built on it would be vacuous.
        // A zoom leaves a per-pixel residual an integer MV cannot remove, which
        // is what raises `average_me_sad` past the gate.
        //
        // The arithmetic is INTEGER throughout (no `f64`, no libm) so the two
        // ISAs generate byte-identical .yuv files; both encoders then consume
        // that one file exactly as before. At the default 1/1 the mapping
        // reduces to `sc = c.saturating_sub(dx)`, i.e. the previous behaviour
        // byte for byte — verified by regenerating the campaign cells.
        let (zoom_num, zoom_den) = spec.zoom.unwrap_or((1, 1));
        // Round-half-away-from-zero integer division — no float, no libm.
        let div_round = |n: i64, d: i64| -> i64 {
            debug_assert!(d > 0);
            if n >= 0 {
                (2 * n + d) / (2 * d)
            } else {
                -((-2 * n + d) / (2 * d))
            }
        };
        // Translate a plane right by `dx` and scale it about the plane centre
        // by `(num/den)^f`, replicating the edges.
        let warp =
            |src: &[u8], pw: usize, ph: usize, dx: usize, num_pow: i64, den_pow: i64| -> Vec<u8> {
                let mut out = vec![0u8; pw * ph];
                // The centre in the SAME half-open convention for both axes.
                let cx = (pw as i64 - 1) / 2;
                let cy = (ph as i64 - 1) / 2;
                for r in 0..ph {
                    let sy = if num_pow == den_pow {
                        r as i64
                    } else {
                        (cy + div_round((r as i64 - cy) * den_pow, num_pow)).clamp(0, ph as i64 - 1)
                    };
                    for c in 0..pw {
                        let cx0 = c as i64 - dx as i64;
                        let sx = if num_pow == den_pow {
                            cx0.clamp(0, pw as i64 - 1)
                        } else {
                            (cx + div_round((cx0 - cx) * den_pow, num_pow)).clamp(0, pw as i64 - 1)
                        };
                        out[r * pw + c] = src[sy as usize * pw + sx as usize];
                    }
                }
                out
            };
        let frame_len_in = w * h + 2 * cw * ch;
        let mut yuv = Vec::with_capacity(n_frames * frame_len_in);
        if let Some(seq) = &rawseq {
            // Real footage: take the frames as they are. The warp, the shift
            // and the zoom are all bypassed — `SVTAV1_FRAME_SHIFT` /
            // `_ZOOM_NUM` / `_ZOOM_DEN` describe a synthetic motion model and
            // silently applying one on top of real motion would make the cell
            // describe neither. A caller that sets both is asking for two
            // different things, so say so instead of picking one.
            assert!(
                spec.frame_shift.is_none() && spec.zoom.is_none(),
                "rawseq: carries its own motion; SVTAV1_FRAME_SHIFT/_ZOOM_NUM/_ZOOM_DEN \
                 apply only to the synthetic warp"
            );
            let have = seq.len() / frame_len_in;
            assert!(
                have >= n_frames,
                "rawseq has {have} frames, SVTAV1_FRAMES asks for {n_frames}"
            );
            yuv.extend_from_slice(&seq[..n_frames * frame_len_in]);
        } else {
            for f in 0..n_frames {
                let dx = shift_px * f;
                if f == 0 {
                    yuv.extend_from_slice(&y);
                    yuv.extend_from_slice(&u);
                    yuv.extend_from_slice(&v);
                } else {
                    let (mut np, mut dp) = (1i64, 1i64);
                    for _ in 0..f {
                        np *= zoom_num;
                        dp *= zoom_den;
                    }
                    yuv.extend_from_slice(&warp(&y, w, h, dx, np, dp));
                    yuv.extend_from_slice(&warp(&u, cw, ch, dx / 2, np, dp));
                    yuv.extend_from_slice(&warp(&v, cw, ch, dx / 2, np, dp));
                }
            }
        }
        // The `.yuv` a differential harness hands to the C driver must be the
        // SAMPLES THIS ENCODER SAW, not the 8-bit planes they were built from.
        // At `bd > 8` the loop below widens each frame with `low_bits`, so
        // writing the u8 buffer here would have given C a different source and
        // called the resulting divergence a port defect. 16-bit little-endian,
        // which is what `capture_c_trace <...> <bit_depth>` reads.
        if bd > 8 {
            let sh = (bd - 8) as u32;
            let mut wide_yuv: Vec<u8> = Vec::with_capacity(yuv.len() * 2);
            for f in 0..n_frames {
                let base = f * frame_len_in;
                let planes: [(usize, usize, usize); 3] = [
                    (base, w * h, w),
                    (base + w * h, cw * ch, cw),
                    (base + w * h + cw * ch, cw * ch, cw),
                ];
                for (off, len, pw) in planes {
                    for (i, &s) in yuv[off..off + len].iter().enumerate() {
                        let v = ((s as u16) << sh) | low_bits(s as usize, i / pw, i % pw);
                        wide_yuv.extend_from_slice(&v.to_le_bytes());
                    }
                }
            }
            std::fs::write(format!("{prefix}.yuv"), &wide_yuv).expect("write .yuv");
        } else {
            std::fs::write(format!("{prefix}.yuv"), &yuv).expect("write .yuv");
        }

        // The GOP: only frame 0 is a key frame unless the caller says
        // otherwise, matching the C driver's SVT_INTRA_PERIOD/-1 default for
        // the first inter cell.
        let intra_period = spec.intra_period;
        // `SVTAV1_HIER_LEVELS` (C driver's `SVT_HIER_LEVELS`): unset means
        // the LIBRARY default, which is C's `HIERARCHICAL_LEVELS_AUTO` — not
        // flat. An unset-env cell must match the C driver's untouched
        // `cfg.hierarchical_levels`, which `enc_handle.c:4556` resolves to
        // 3 on low delay / 4 or 5 on random access; defaulting to 0 here
        // silently encoded a flat stream where C built a pyramid
        // (found 2026-09-25: the "RA emission-order divergence" was this).
        let hier = spec.hier_levels;
        let rc = RcConfig {
            // `SVTAV1_RC_MODE` (C's `--rc`): 0 = CQP, 1 = VBR, 2 = CBR.
            // Default 0 keeps every existing cell. CBR is admitted only
            // under LOW_DELAY (`SVT_PRED_STRUCT` unset/1) — C's own
            // envelope (enc_settings.c:157); the pipeline refuses the
            // rest.
            mode: spec.rc_mode,
            qp,
            // `SVT_AQ_MODE` (C's `--aq-mode`): 2 selects the TPL-gated
            // per-SB deltaq — live on the random-access path, inert on
            // low-delay, exactly as `get_tpl` gates it in C
            // (enc_handle.c:3657). Default 0 keeps every existing cell.
            aq_mode: spec.aq_mode,
            // `SVTAV1_TBR` (C's `--tbr`, kbps) — the CBR/VBR target.
            target_bitrate: spec.tbr,
            // `SVTAV1_VBV` (C's `--vbv-bufsize`, ms) — max buffer size;
            // starting/optimal levels take C's defaults (600/600).
            buffer_size_ms: spec.vbv,
            // `SVTAV1_FPS` — the framerate RC scales bitrates by. C's
            // capture pins 30/1; match it unless overridden.
            framerate: spec.fps,
            ..RcConfig::default()
        };
        let mono = spec.mono;
        let mut pipeline =
            EncodePipeline::new_with_preset(w as u32, h as u32, preset, rc, hier, intra_period)
                .with_bit_depth(bd)
                .with_sb_size(spec.sb);
        // `SVT_PRED_STRUCT` (C's `-pred-struct`): 1 = low delay (default),
        // 2 = random access — buffers a mini-GOP and emits in decode order.
        if spec.random_access {
            pipeline = pipeline
                .with_pred_structure(svtav1_encoder::port_picstruct::PredStructure::RandomAccess);
        }
        // `SVT_ENABLE_TF` (C's `--enable-tf`): 0 disables motion-compensated
        // temporal filtering on the random-access path.
        if !spec.enable_tf {
            pipeline.enable_tf = false;
        }
        if let Some(d) = timeout {
            pipeline = pipeline.with_timeout(d);
        }
        if !mono {
            pipeline = pipeline.with_chroma_420(true);
        }
        // Only when SVTAV1_FINAL_RECON asks for the per-frame dump below.
        // Byte-INERT here by construction: `recon_output` reaches the encode
        // through `postfilter_consumed` (pipeline.rs), which a multi-frame run
        // already forces true via `!is_single_frame` — a later frame may
        // predict from this recon, so the filters run either way. Everything
        // else it does is a clone into `last_recon*`.
        if spec.dumps.final_recon.is_some() {
            pipeline = pipeline.with_recon_output(true);
        }
        // Grain, `SVT_ORACLE` / `SVT_HDR_MODE` / `SVT_FORK_*`, `SVTAV1_TUNE`
        // and the enhancements: the same calls the still path makes, so a
        // multi-frame cell cannot silently encode without one of them (the RA
        // path once ignored the fork knobs, and a "tune-VQ" RA cell once
        // compared C-VQ against port-PSNR).
        spec.apply_common(&mut pipeline);
        let frame_len = w * h + 2 * cw * ch;
        let mut all = Vec::new();
        for f in 0..n_frames {
            let base = f * frame_len;
            let fy = &yuv[base..base + w * h];
            let fu = &yuv[base + w * h..base + w * h + cw * ch];
            let fv = &yuv[base + w * h + cw * ch..base + frame_len];
            let r = if bd > 8 {
                // Widen exactly as the single-frame bd10 path does, so a
                // multi-frame 10-bit cell differs from its 8-bit twin only in
                // depth.
                let sh = (bd - 8) as u32;
                let wide = |p: &[u8], pw: usize| -> Vec<u16> {
                    p.iter()
                        .enumerate()
                        .map(|(i, &s)| ((s as u16) << sh) | low_bits(s as usize, i / pw, i % pw))
                        .collect()
                };
                let (y16, u16v, v16) = (wide(fy, w), wide(fu, cw), wide(fv, cw));
                if mono {
                    pipeline.try_encode_frame_hbd(&y16, w)
                } else {
                    pipeline.try_encode_frame_420_hbd(&y16, &u16v, &v16, w)
                }
            } else if mono {
                pipeline.try_encode_frame(fy, w)
            } else {
                pipeline.try_encode_frame_420(fy, fu, fv, w)
            };
            match r {
                Ok(bytes) => {
                    // Per-frame file alongside the concatenation, so a differ
                    // can compare ONE frame instead of a stream whose first
                    // divergence is just "frame 0 got longer".
                    std::fs::write(format!("{prefix}.obu.f{f}"), &bytes).expect("write frame obu");
                    if let Some(path) = &spec.dumps.final_recon {
                        dump::final_frames(path, &mut pipeline);
                        if bd == 10 {
                            dump::final_frame10(path, &pipeline);
                        }
                    }
                    // Coded-frame display order; `map_or(f)` keeps the
                    // low-delay naming.
                    let dorder = pipeline.last_recon_display_order.map_or(f, |d| d as usize);
                    if let Some(pfx) = &spec.dumps.recon_stages {
                        dump::stages(pfx, &pipeline, bd, dorder);
                    }
                    all.extend_from_slice(&bytes);
                }
                Err(e) => {
                    // Write what DID encode, then exit — 3 is the established
                    // "correctly refused, did not crash" code; 4 names a
                    // fired stop token (see unwrap_or_refuse). A gate reading
                    // this can name the frame that stopped the sequence.
                    std::fs::write(format!("{prefix}.obu"), &all).expect("write .obu");
                    if matches!(e.error(), svtav1_encoder::EncodeError::Cancelled(_)) {
                        eprintln!("identity_run: STOPPED by the stop token at frame {f}: {e}");
                        std::process::exit(4);
                    }
                    eprintln!("identity_run: REFUSED by the encoder at frame {f}: {e}");
                    std::process::exit(3);
                }
            }
        }
        // Random access buffers a mini-GOP before emitting; the trailing
        // partial window comes out of `try_flush`, not a frame call.
        match pipeline.try_flush() {
            Ok(bytes) => all.extend_from_slice(&bytes),
            Err(e) => {
                std::fs::write(format!("{prefix}.obu"), &all).expect("write .obu");
                eprintln!("identity_run: REFUSED by the encoder at flush: {e}");
                std::process::exit(3);
            }
        }
        // The trailing partial window's coded frames surface here, not in a
        // frame call; drain their recons too.
        if let Some(path) = &spec.dumps.final_recon {
            dump::final_frames(path, &mut pipeline);
        }
        std::fs::write(format!("{prefix}.obu"), &all).expect("write .obu");
        // The recon dumps below are single-frame constructs; a multi-frame run
        // deliberately stops here rather than dumping a last-frame recon under
        // a name that reads like the whole sequence.
        return;
    }

    if bd > 8 {
        let shift = (bd - 8) as u32;
        let mut yuv = Vec::with_capacity((w * h + 2 * cw * ch) * 2);
        if hbd_src {
            for &s in y10.iter().chain(u10.iter()).chain(v10.iter()) {
                yuv.extend_from_slice(&s.to_le_bytes());
            }
        } else {
            for &s in y.iter().chain(u.iter()).chain(v.iter()) {
                yuv.extend_from_slice(&(((s as u16) << shift).to_le_bytes()));
            }
        }
        std::fs::write(format!("{prefix}.yuv"), &yuv).expect("write .yuv");
    } else {
        let mut yuv = Vec::with_capacity(w * h * 3 / 2);
        yuv.extend_from_slice(&y);
        yuv.extend_from_slice(&u);
        yuv.extend_from_slice(&v);
        std::fs::write(format!("{prefix}.yuv"), &yuv).expect("write .yuv");
    }

    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp, // CLI domain 0..63, same as the C driver's cfg.qp
        // `SVT_AQ_MODE` (C's `--aq-mode`) — see the multi-frame block above.
        // Inert on a still either way (TPL is off without lookahead).
        aq_mode: spec.aq_mode,
        ..RcConfig::default()
    };
    let mono = spec.mono;
    let mut pipeline = EncodePipeline::new_with_preset(w as u32, h as u32, preset, rc, 0, 1)
        .with_tile_rows_log2(spec.tile_rows_log2)
        .with_tile_cols_log2(spec.tile_cols_log2)
        .with_bit_depth(bd)
        .with_sb_size(spec.sb)
        // The recon dumps below (SVTAV1_RECON_BIN and friends) read
        // last_recon*, which is opt-in since the post-filter passes that
        // produce it are byte-inert at preset >= 7.
        .with_recon_output(true);
    if let Some(d) = timeout {
        pipeline = pipeline.with_timeout(d);
    }
    if let Some(d) = spec.superres {
        pipeline = pipeline.with_superres(d);
        if let Some(path) = &spec.dumps.sr_dump {
            dump::superres_source(path, &pipeline, (&y, &u, &v), w, h);
        }
    }
    let sb128_fallback = pipeline.sb128_fallback;
    let sb_size_used = pipeline.sb_size;
    spec.apply_common(&mut pipeline);
    spec.apply_still(&mut pipeline);
    // SVTAV1_Y_STRIDE=<n> (>= w): hand the LUMA plane to the encoder at a
    // stride WIDER than the frame, with the slack POISONED (0xA5 / 0x0A5A5).
    //
    // The project's pixel-buffer rule is that any multi-row function handles
    // `stride != width`, and the encoder's public entry points take a luma
    // stride — but no gate ever passed one that differed. The #15 intra-clamp
    // defect turned on exactly that confusion in the other direction
    // (`frame_h = y_recon.len() / y_stride` treated a buffer's shape as the
    // frame's extent), so "a padded stride never changes the bitstream" is
    // worth PROVING rather than assuming.
    //
    // Poison, not edge-replication: replicating would make a stray read of the
    // padding return a plausible value and hide the bug. The `.yuv` the C
    // oracle reads stays tightly packed, so a cell that byte-matches C at a
    // padded stride has proven both halves at once.
    if let Some(s) = spec.y_stride {
        assert!(
            s >= w,
            "SVTAV1_Y_STRIDE {s} is narrower than the frame ({w})"
        );
    }
    let y_stride = spec.y_stride.unwrap_or(w);
    let restride = |p: &[u8], pw: usize, ph: usize| -> Vec<u8> {
        if y_stride == pw {
            return p.to_vec();
        }
        let mut o = vec![0xA5u8; y_stride * ph];
        for r in 0..ph {
            o[r * y_stride..r * y_stride + pw].copy_from_slice(&p[r * pw..r * pw + pw]);
        }
        o
    };
    let y_in = restride(&y, w, h);
    let y10_in: Vec<u16> = if hbd_src && y_stride != w {
        let mut o = vec![0xA5A5u16; y_stride * h];
        for r in 0..h {
            o[r * y_stride..r * y_stride + w].copy_from_slice(&y10[r * w..r * w + w]);
        }
        o
    } else {
        y10.clone()
    };
    let (y, y10) = (y_in, y10_in);

    let obu = if hbd_src {
        // Task #6: the native-10-bit entry points — the port sees the SAME
        // real u16 samples written to the .yuv the C oracle reads.
        if mono {
            pipeline
                .try_encode_frame_hbd(&y10, y_stride)
                .expect("hbd mono encode inside the documented envelope")
        } else {
            pipeline = pipeline.with_chroma_420(true);
            pipeline
                .try_encode_frame_420_hbd(&y10, &u10, &v10, y_stride)
                .expect("hbd 4:2:0 encode inside the documented envelope")
        }
    } else if mono {
        unwrap_or_refuse(pipeline.try_encode_frame(&y, y_stride))
    } else {
        pipeline = pipeline.with_chroma_420(true);
        unwrap_or_refuse(pipeline.try_encode_frame_420(&y, &u, &v, y_stride))
    };
    std::fs::write(format!("{prefix}.obu"), &obu).expect("write .obu");

    if let Some(pfx) = &spec.dumps.recon_dump {
        dump::recon_dump(pfx, &pipeline);
    }
    if let Some(path) = &spec.dumps.final_recon {
        dump::final_still(path, &pipeline, bd);
    }
    if let Some(path) = &spec.dumps.bd10_recon {
        dump::bd10_luma(path, &pipeline);
    }
    // Status goes to STDOUT, never stderr: identity_diff.sh captures stderr
    // verbatim into `rs.trace` (the symtrace op stream the differ parses).
    println!(
        "identity_run: {content} {w}x{h} qp={qp} preset={preset} sb={} -> {} bytes",
        sb_size_used,
        obu.len()
    );
    if sb128_fallback {
        // Loud, so a gate cell can never silently "pass" while the port
        // quietly coded a different superblock geometry than C did.
        println!(
            "identity_run: SB128-FALLBACK — C codes {w}x{h} preset {preset} with 128px \
             superblocks (sb128_geom::derive_super_block_size); this port emitted a valid \
             64px-SB stream that will NOT byte-match."
        );
    }
}

/// Turn a pipeline refusal into a DISTINCT exit status instead of a panic.
///
/// The encoder deliberately REFUSES configurations it cannot encode faithfully
/// (unsupported bit depth, qp 0 / lossless, an inter frame, an out-of-envelope
/// superres) rather than emit a plausible-but-wrong stream. Going through the
/// infallible `encode_frame*` wrappers turned every one of those into a panic
/// via their `.expect()`, which is indistinguishable from a real crash to a
/// harness — `tools/arbitrary_size_robustness.sh` reported 48 refusals as
/// PANIC. Exit code 3 lets a gate tell "correctly refused" from "crashed".
/// Exit 4 is the stop-token arm: the SVTAV1_TIMEOUT_MS deadline (or any
/// installed `Stop`) fired — a wedged encode surfacing as a named exit
/// rather than a silent kill or an endless spin.
fn unwrap_or_refuse(r: svtav1_encoder::EncodeResult<Vec<u8>>) -> Vec<u8> {
    match r {
        Ok(v) => v,
        Err(e) => {
            if matches!(e.error(), svtav1_encoder::EncodeError::Cancelled(_)) {
                eprintln!("identity_run: STOPPED by the stop token: {e}");
                std::process::exit(4);
            }
            eprintln!("identity_run: REFUSED by the encoder: {e}");
            std::process::exit(3);
        }
    }
}
