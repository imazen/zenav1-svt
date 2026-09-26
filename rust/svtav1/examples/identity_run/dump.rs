//! Recon and source dumps. None of them changes a coded byte.
//!
//! The FINAL recon dumps are the oracle-free half of the alignment gates:
//! byte-identity to C cannot see an encoder/decoder recon MISMATCH, and an
//! inter frame predicts from this buffer, so a reference that is not what a
//! decoder reconstructs makes every later mode decision score a prediction
//! the stream does not describe (#15's intra-reference clamp was one).
//! Each is cropped to the true coded dims (ceiling chroma) and tightly packed
//! Y|U|V, byte-comparable with `aomdec`/`dav1d` output for the same frame.
//!
//! AT 10 BITS THE 10-BIT CANVAS IS THE ONE A DECODER OUTPUTS. A frame without
//! a complete 10-bit recon is refused rather than dumped from the u8 chain:
//! the multi-frame writer once did that, and every bd10 comparison against
//! `aomdec` read as a total mismatch from frame 0.

use svtav1_encoder::pipeline::EncodePipeline;

const NO_RECON10: &str = "SVTAV1_FINAL_RECON at bd10: last_recon10_final is None — this frame \
     produced no complete 10-bit recon (out of the bd10 envelope), so there is no 10-bit final \
     recon to dump; refusing to write the u8 chain's recon in its place";

/// The dump crop: `aw` is the recon stride, (`tw`, `th`) what a decoder
/// outputs, `fmt` the chroma geometry (full-resolution at 4:4:4).
struct Geom {
    aw: usize,
    tw: usize,
    th: usize,
    fmt: svtav1_types::chroma::ChromaFormat,
}

impl Geom {
    fn of(p: &EncodePipeline) -> Self {
        Self {
            aw: p.width as usize,
            tw: p.true_width as usize,
            th: p.true_height as usize,
            fmt: p
                .chroma_format
                .unwrap_or(svtav1_types::chroma::ChromaFormat::Yuv420),
        }
    }

    /// A superres still's final recon is already upscaled to the output width.
    fn of_final_still(p: &EncodePipeline) -> Self {
        if p.superres_denom.is_some() {
            let uw = p.upscaled_width as usize;
            Self {
                aw: uw,
                tw: uw,
                th: p.true_height as usize,
                fmt: p
                    .chroma_format
                    .unwrap_or(svtav1_types::chroma::ChromaFormat::Yuv420),
            }
        } else {
            Self::of(p)
        }
    }

    /// Y, then U and V unless the frame is monochrome (empty chroma).
    /// Chroma dims are the format's: ceiling-half at 4:2:0, FULL-resolution
    /// at 4:4:4 (where a decoder's U/V planes are `tw`x`th`).
    fn pack<T: Copy>(&self, y: &[T], u: &[T], v: &[T]) -> Vec<T> {
        let (acw, tcw, tch) = (
            self.fmt.chroma_width(self.aw),
            self.fmt.chroma_width(self.tw),
            self.fmt.chroma_height(self.th),
        );
        let mut o = Vec::with_capacity(self.tw * self.th + 2 * tcw * tch);
        crop_into(&mut o, y, self.aw, self.tw, self.th);
        if !u.is_empty() {
            crop_into(&mut o, u, acw, tcw, tch);
            crop_into(&mut o, v, acw, tcw, tch);
        }
        o
    }
}

fn crop_into<T: Copy>(o: &mut Vec<T>, p: &[T], stride: usize, cw: usize, chh: usize) {
    for r in 0..chh {
        o.extend_from_slice(&p[r * stride..r * stride + cw]);
    }
}

fn le(s: &[u16]) -> Vec<u8> {
    s.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// Multi-frame: write `<path>.f<display order>` for every coded frame the
/// last call produced. Under random access one input call can code a whole
/// mini-GOP, and a hidden base layer's recon lands on the display index it
/// is shown at, exactly where a decoder's output puts it. Called after
/// `try_flush` too, or the last mini-GOP's recons would be dropped.
pub fn final_frames(path: &str, p: &mut EncodePipeline) {
    let g = Geom::of(p);
    while let Some((dorder, (ry, ru, rv))) = p.recon_frames.pop_front() {
        std::fs::write(format!("{path}.f{dorder}"), g.pack(&ry, &ru, &rv))
            .expect("write SVTAV1_FINAL_RECON");
    }
}

/// Multi-frame at 10 bits: the frame's 10-bit final recon, replacing the u8
/// file [`final_frames`] wrote under the same name.
pub fn final_frame10(path: &str, p: &EncodePipeline) {
    let (ry, ru, rv) = p.last_recon10_final.as_ref().expect(NO_RECON10);
    let dorder = p
        .last_recon_display_order
        .expect("recon_output set produced a coded frame");
    std::fs::write(
        format!("{path}.f{dorder}"),
        le(&Geom::of(p).pack(ry, ru, rv)),
    )
    .expect("write SVTAV1_FINAL_RECON");
}

/// Multi-frame `SVTAV1_RECON_STAGES=<pfx>`: the pre-deblock recon,
/// `<pfx>.f<i>.pre.bin`, and at 10 bits the 10-bit canvas as `.pre10.bin`.
///
/// This split localises a recon mismatch: `dav1d --inloopfilters none`
/// reconstructs prediction + residual and stops. If THAT equals this dump,
/// prediction, residual and reference are correct and the divergence is in
/// the filters; if not, the filters are innocent.
pub fn stages(pfx: &str, p: &EncodePipeline, bd: u8, dorder: usize) {
    let g = Geom::of(p);
    if let Some((py, pu, pv)) = p.last_recon_unfiltered.as_ref() {
        std::fs::write(format!("{pfx}.f{dorder}.pre.bin"), g.pack(py, pu, pv))
            .expect("write SVTAV1_RECON_STAGES");
    }
    if bd == 10
        && let (Some(py), Some((pu, pv))) = (p.last_recon10_y.as_ref(), p.last_recon10_uv.as_ref())
    {
        std::fs::write(
            format!("{pfx}.f{dorder}.pre10.bin"),
            le(&g.pack(py, pu, pv)),
        )
        .expect("write SVTAV1_RECON_STAGES 10-bit");
    }
}

/// Still `SVTAV1_FINAL_RECON=<path>`: the final recon; packed u16 LE at bd10,
/// comparable with aomdec's `C420p10` y4m.
pub fn final_still(path: &str, p: &EncodePipeline, bd: u8) {
    let g = Geom::of_final_still(p);
    let b = if bd == 10 {
        let (ry, ru, rv) = p.last_recon10_final.as_ref().expect(NO_RECON10);
        le(&g.pack(ry, ru, rv))
    } else {
        let (ry, ru, rv) = p
            .last_recon
            .as_ref()
            .expect("with_recon_output(true) is set");
        g.pack(ry, ru, rv)
    };
    std::fs::write(path, &b).expect("write SVTAV1_FINAL_RECON");
}

/// Still `SVTAV1_RECON_DUMP=<pfx>`: uncropped pre- and post-deblock recon,
/// the layout of the instrumented C `dlf_process` dump.
pub fn recon_dump(pfx: &str, p: &EncodePipeline) {
    for (name, r) in [
        ("pre", &p.last_recon_unfiltered),
        ("post", &p.last_recon_pre_cdef),
    ] {
        if let Some((yy, uu, vv)) = r {
            let b = [yy.as_slice(), uu, vv].concat();
            std::fs::write(format!("{pfx}.{name}.bin"), &b).expect("write recon dump");
            eprintln!(
                "SVTAV1_RECON_DUMP {name} -> {pfx}.{name}.bin ({} bytes)",
                b.len()
            );
        }
    }
}

/// Still `SVTAV1_BD10_RECON=<path>`: the re-encode pass's 10-bit luma recon
/// (u16 LE), for the self-consistency check against a decoder's prefilter.
pub fn bd10_luma(path: &str, p: &EncodePipeline) {
    if let Some(r10) = p.last_recon10_y.as_ref() {
        std::fs::write(path, le(r10)).expect("write recon10");
        eprintln!("SVTAV1_BD10_RECON -> {path} ({} u16)", r10.len());
    }
}

/// Still `SVTAV1_SR_DUMP=<path>`: the DOWNSCALED I420 source a superres
/// encode codes (the same `resize_plane_horizontal`, pinned byte-exact vs C),
/// so a superres divergence can be split into "the coded-width encode of this
/// content" (re-run via `raw:` without superres) and "a statistic C derives
/// before scaling".
pub fn superres_source(
    path: &str,
    p: &EncodePipeline,
    planes: (&[u8], &[u8], &[u8]),
    w: usize,
    h: usize,
) {
    use svtav1_dsp::resize::resize_plane_horizontal;
    let (y, u, v) = planes;
    let cwid = p.true_width as usize;
    let (ucw, uch) = (w.div_ceil(2), h.div_ceil(2));
    let ccw = cwid.div_ceil(2);
    let mut yd = vec![0u8; cwid * h];
    let mut ud = vec![0u8; ccw * uch];
    let mut vd = vec![0u8; ccw * uch];
    resize_plane_horizontal(y, h, w, w, &mut yd, cwid, cwid);
    resize_plane_horizontal(u, uch, ucw, ucw, &mut ud, ccw, ccw);
    resize_plane_horizontal(v, uch, ucw, ucw, &mut vd, ccw, ccw);
    std::fs::write(path, [yd, ud, vd].concat()).expect("write SVTAV1_SR_DUMP");
    eprintln!("SVTAV1_SR_DUMP {cwid}x{h} -> {path}");
}
