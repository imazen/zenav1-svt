//! RGBA in, AVIF out — the shortest path from decoded pixels to a file.
//!
//! Everything else in this crate speaks planar 4:2:0 because that is what the
//! encoder codes. Callers arriving from a decoder (GIF, PNG, an MP4's video
//! track) have interleaved RGBA, and the conversion between the two is the
//! same handful of steps every time: BT.601 limited-range luma/chroma, 2x2
//! chroma averaging, and an alpha plane only when some pixel is not opaque.
//!
//! This module is that conversion plus the two entry points that use it, so a
//! caller converting an animation writes a loop over their decoder and one
//! call rather than a colour-space implementation.
//!
//! # Colour
//!
//! BT.601 limited range (Y 16..235, chroma 16..240), which is what
//! [`crate::avif::AvifEncoder`]'s default CICP describes. Set a different
//! colour space on the encoder and the SAMPLES here still carry BT.601
//! primaries — the CICP would then be describing something the samples are
//! not. Convert yourself and use the planar entry points if you need another
//! matrix.

use crate::avif::animation::{AnimationFrame, AnimationTiming};
use crate::avif::{AvifEncoder, EncodeError};

/// One RGBA frame of an animation, and how long it is shown.
#[derive(Debug, Clone, Copy)]
pub struct RgbaFrame<'a> {
    /// Row-major RGBA8, tightly packed: `width * height * 4` bytes.
    pub rgba: &'a [u8],
    /// How long this frame shows, in milliseconds. Zero is treated as one
    /// millisecond — a zero-duration frame is not representable in a timed
    /// track, and silently dropping it would lose a frame of the animation.
    pub duration_ms: u32,
}

/// The planar form of one RGBA frame.
struct Planar {
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
    alpha: Option<Vec<u8>>,
}

/// BT.601 limited-range RGB -> Y'CbCr with 2x2 chroma averaging, plus the
/// alpha plane when the image actually uses it.
///
/// The chroma average is over the four source pixels of each 2x2 group, with
/// odd edges repeating the last row/column — the same rule the rest of this
/// port uses when it subsamples.
fn to_planar(rgba: &[u8], width: usize, height: usize) -> Planar {
    let mut y = vec![0u8; width * height];
    let cw = width.div_ceil(2);
    let ch = height.div_ceil(2);
    let mut u = vec![0u8; cw * ch];
    let mut v = vec![0u8; cw * ch];
    let mut any_transparent = false;

    for row in 0..height {
        for col in 0..width {
            let p = (row * width + col) * 4;
            let (r, g, b) = (
                i32::from(rgba[p]),
                i32::from(rgba[p + 1]),
                i32::from(rgba[p + 2]),
            );
            any_transparent |= rgba[p + 3] != 255;
            // BT.601: Y = 16 + (65.738 R + 129.057 G + 25.064 B) / 256, in
            // the fixed-point form the rest of the harness uses.
            y[row * width + col] =
                (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16).clamp(0, 255) as u8;
        }
    }
    for cr in 0..ch {
        for cc in 0..cw {
            let (mut sr, mut sg, mut sb, mut n) = (0i32, 0i32, 0i32, 0i32);
            for dy in 0..2 {
                for dx in 0..2 {
                    let row = (cr * 2 + dy).min(height - 1);
                    let col = (cc * 2 + dx).min(width - 1);
                    let p = (row * width + col) * 4;
                    sr += i32::from(rgba[p]);
                    sg += i32::from(rgba[p + 1]);
                    sb += i32::from(rgba[p + 2]);
                    n += 1;
                }
            }
            let (r, g, b) = (sr / n, sg / n, sb / n);
            u[cr * cw + cc] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
            v[cr * cw + cc] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
        }
    }
    // An alpha plane costs a whole extra AV1 track, so it is written only when
    // the image uses it. A fully opaque RGBA input produces the same file a
    // caller would get from RGB.
    let alpha = any_transparent.then(|| {
        (0..width * height)
            .map(|i| rgba[i * 4 + 3])
            .collect::<Vec<u8>>()
    });
    Planar { y, u, v, alpha }
}

impl AvifEncoder {
    /// Encode RGBA8 frames as an animated AVIF, ready to write to a file.
    ///
    /// Durations are milliseconds; the track runs at a 1000 Hz timescale so
    /// they are carried exactly rather than rounded into a coarser tick.
    ///
    /// ```no_run
    /// # use svtav1::{avif::AvifEncoder, rgba::RgbaFrame};
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let (w, h) = (64u32, 64u32);
    /// # let decoded: Vec<(Vec<u8>, u32)> = vec![];
    /// let frames: Vec<RgbaFrame> = decoded
    ///     .iter()
    ///     .map(|(rgba, ms)| RgbaFrame { rgba, duration_ms: *ms })
    ///     .collect();
    /// let avif = AvifEncoder::new().encode_rgba_animation(&frames, w, h)?;
    /// std::fs::write("out.avif", &avif)?;
    /// # Ok(()) }
    /// ```
    pub fn encode_rgba_animation(
        &self,
        frames: &[RgbaFrame<'_>],
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, EncodeError> {
        if frames.is_empty() {
            return Err(EncodeError::InvalidDimensions {
                width,
                height,
                reason: "an animation needs at least one frame",
            });
        }
        let (w, h) = (width as usize, height as usize);
        let want = w.checked_mul(h).and_then(|n| n.checked_mul(4));
        let mut planes = Vec::with_capacity(frames.len());
        for f in frames {
            if want != Some(f.rgba.len()) {
                return Err(EncodeError::InvalidDimensions {
                    width,
                    height,
                    reason: "every frame must be width * height * 4 bytes",
                });
            }
            planes.push(to_planar(f.rgba, w, h));
        }
        // An alpha track is per-SEQUENCE, not per-frame: if any frame uses
        // alpha they all carry a plane, or the track would be ragged.
        let any_alpha = planes.iter().any(|p| p.alpha.is_some());
        let opaque = vec![255u8; w * h];
        let anim: Vec<AnimationFrame<'_>> = planes
            .iter()
            .zip(frames)
            .map(|(p, f)| AnimationFrame {
                y: &p.y,
                u: &p.u,
                v: &p.v,
                y_stride: w,
                alpha: any_alpha.then(|| p.alpha.as_deref().unwrap_or(&opaque)),
                duration: f.duration_ms.max(1),
            })
            .collect();
        self.encode_animation_yuv420(&anim, width, height, AnimationTiming { timescale: 1000 })
    }
}
