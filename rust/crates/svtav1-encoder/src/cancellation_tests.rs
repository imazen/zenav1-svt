//! Deterministic cancellation inside post-filter work, independent of wall time.
use crate::{EncodeError, EncodeResult, cdef, deblock, restoration};
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

struct StopAfter(AtomicUsize, usize);
impl enough::Stop for StopAfter {
    fn check(&self) -> Result<(), enough::StopReason> {
        if self.0.fetch_add(1, Ordering::Relaxed) >= self.1 {
            Err(enough::StopReason::Cancelled)
        } else {
            Ok(())
        }
    }
    fn may_stop(&self) -> bool {
        true
    }
}

// Count a successful pass, then cancel at EVERY visited checkpoint. Requiring
// multiple checks rejects an implementation that only checks at function entry.
fn exercise<T>(minimum_checks: usize, mut run: impl FnMut(&dyn enough::Stop) -> EncodeResult<T>) {
    let count = StopAfter(AtomicUsize::new(0), usize::MAX);
    assert!(run(&count).is_ok());
    let checks = count.0.load(Ordering::Relaxed);
    assert!(
        checks >= minimum_checks,
        "only {checks} checkpoints, need {minimum_checks}"
    );
    for n in 0..checks {
        let stop = StopAfter(AtomicUsize::new(0), n);
        assert!(
            matches!(run(&stop), Err(e) if matches!(e.error(), EncodeError::Cancelled(enough::StopReason::Cancelled))),
            "checkpoint {n}/{checks} ignored"
        );
        assert_eq!(
            stop.0.load(Ordering::Relaxed),
            n + 1,
            "work continued polling after cancellation"
        );
    }
}

fn pixels(w: usize, h: usize) -> Vec<u8> {
    (0..w * h)
        .map(|i| (96 + (i * 7 + i / w * 3) % 32) as u8)
        .collect()
}

#[test]
fn cdef_apply_and_search_cancel_within_frame_at_both_depths() {
    let (w, h) = (64, 128);
    let mut geom = deblock::DeblockGeom::new(w, h, w, h);
    for y in (0..h).step_by(8) {
        for x in (0..w).step_by(8) {
            geom.record_block(x, y, 8, 8, false, false);
        }
    }
    let src = pixels(w, h);
    let src10: Vec<u16> = src.iter().map(|&v| u16::from(v) * 4).collect();
    let pick = cdef::CdefPick::single(cdef::CdefFrameParams {
        damping: 4,
        y_strength: 20,
        uv_strength: 0,
    });
    let cfg = cdef::CdefSearchCfg {
        fs: vec![0, 20],
        first_pass_num: 2,
        subsampling: 1,
        zero_fs_cost_bias: 0,
    };
    exercise(3, |stop| {
        cdef::apply_cdef_frame_with_stop(
            &mut src.clone(),
            &mut [],
            &mut [],
            w,
            h,
            false,
            &geom,
            &pick,
            stop,
        )
    });
    exercise(3, |stop| {
        cdef::apply_cdef_frame_hbd_with_stop(
            &mut src10.clone(),
            &mut [],
            &mut [],
            w,
            h,
            false,
            &geom,
            &pick,
            10,
            stop,
        )
    });
    exercise(3, |stop| {
        cdef::cdef_search_still_with_stop(
            &cfg,
            &src,
            &[],
            &[],
            &src,
            &[],
            &[],
            w,
            h,
            false,
            &geom,
            160,
            stop,
        )
    });
    exercise(3, |stop| {
        cdef::cdef_search_still_hbd_with_stop(
            &cfg,
            &src10,
            &[],
            &[],
            &src10,
            &[],
            &[],
            w,
            h,
            false,
            &geom,
            160,
            10,
            stop,
        )
    });
}

#[test]
fn deblock_apply_and_level_search_cancel_within_frame_at_both_depths() {
    let (w, h) = (64, 64);
    let mut geom = deblock::DeblockGeom::new(w, h, w, h);
    for y in (0..h).step_by(8) {
        for x in (0..w).step_by(8) {
            geom.record_block(x, y, 8, 8, false, false);
        }
    }
    let y = pixels(w, h);
    let uv = pixels(w / 2, h / 2);
    let y10: Vec<u16> = y.iter().map(|&v| u16::from(v) * 4).collect();
    let uv10: Vec<u16> = uv.iter().map(|&v| u16::from(v) * 4).collect();
    let levels = deblock::LfLevels {
        levels: [8, 8, 8, 8],
    };
    exercise(4, |stop| {
        deblock::apply_deblock_frame_with_stop(
            &mut y.clone(),
            &mut uv.clone(),
            &mut uv.clone(),
            w,
            h,
            true,
            &geom,
            &levels,
            0,
            stop,
        )
    });
    exercise(4, |stop| {
        deblock::apply_deblock_frame_hbd_with_stop(
            &mut y10.clone(),
            &mut uv10.clone(),
            &mut uv10.clone(),
            w,
            h,
            true,
            &geom,
            &levels,
            0,
            10,
            stop,
        )
    });
    let input = deblock::DlfSearchInput {
        y_src: &y,
        u_src: &uv,
        v_src: &uv,
        y_recon: &y,
        u_recon: &uv,
        v_recon: &uv,
        width: w,
        height: h,
        chroma_420: true,
        geom: &geom,
        early_exit_convergence: 0,
        sharpness: 0,
        bit_depth: 8,
    };
    let input10 = deblock::DlfSearchInput {
        y_src: &y10,
        u_src: &uv10,
        v_src: &uv10,
        y_recon: &y10,
        u_recon: &uv10,
        v_recon: &uv10,
        width: w,
        height: h,
        chroma_420: true,
        geom: &geom,
        early_exit_convergence: 0,
        sharpness: 0,
        bit_depth: 10,
    };
    exercise(3, |stop| {
        deblock::pick_filter_levels_full_image_with_stop(
            &input,
            &deblock::key_frame_pick_inputs(0, 8),
            stop,
        )
    });
    exercise(3, |stop| {
        deblock::pick_filter_levels_full_image_with_stop(
            &input10,
            &deblock::key_frame_pick_inputs(0, 10),
            stop,
        )
    });
}

fn restoration_checks<P: restoration::LrPixel>(src: &[P], bd: u8) {
    let (w, h) = (384, 64);
    let wn = restoration::wn_filter_ctrls_allintra(6);
    let sg = crate::port_lr_level::SgFilterCtrls::default();
    exercise(8, |stop| {
        restoration::search_restoration_still_bd_with_stop(
            &wn,
            &sg,
            src,
            &[],
            &[],
            src,
            &[],
            &[],
            w,
            h,
            false,
            1000,
            bd,
            stop,
        )
    });
    let mut info = restoration::search_restoration_still_bd(
        &wn,
        &sg,
        src,
        &[],
        &[],
        src,
        &[],
        &[],
        w,
        h,
        false,
        1000,
        bd,
    )
    .unwrap();
    // Force a real apply walk: identical source/recon can legitimately choose NONE.
    info.planes[0].frame_rtype = svtav1_dsp::restoration::RESTORE_WIENER;
    for unit in &mut info.planes[0].units {
        unit.rtype = svtav1_dsp::restoration::RESTORE_WIENER;
    }
    let boundaries =
        restoration::save_lr_boundaries_bd(src, &[], &[], src, &[], &[], w, h, w, w / 2, false);
    exercise(6, |stop| {
        restoration::apply_restoration_frame_bd_with_stop(
            &mut src.to_vec(),
            &mut [],
            &mut [],
            w,
            h,
            w,
            w / 2,
            false,
            &info,
            &boundaries,
            bd,
            stop,
        )
    });
}

#[test]
fn restoration_search_and_apply_cancel_between_units_at_both_depths() {
    let src = pixels(384, 64);
    restoration_checks(&src, 8);
    let src10: Vec<u16> = src.iter().map(|&v| u16::from(v) * 4).collect();
    restoration_checks(&src10, 10);
}
