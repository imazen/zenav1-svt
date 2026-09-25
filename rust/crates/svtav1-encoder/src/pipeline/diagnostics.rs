use super::*;

#[inline(always)]
pub(super) fn dump_gm(
    display_order: u64,
    frame_me: &Option<crate::inter_me_arm::FrameMe>,
    gm_estimation: Option<crate::port_global_me::GmEstimation>,
    gm_models: Option<crate::port_global_me::GmModels>,
) {
    if crate::dbgenv::gmdbg() {
        if let Some(g) = gm_estimation.as_ref() {
            eprintln!(
                "GMPORT poc={display_order} b64={} total_me_sad={} avg_me_sad={} \
                     total_gm_sbs={} level={} ds={} searches={} all_identity={}",
                frame_me.as_ref().map_or(0, |m| m.per_b64.len()),
                frame_me.as_ref().map_or(0u32, |m| m
                    .per_b64
                    .iter()
                    .fold(0u32, |a, b| a.wrapping_add(b.rc_me_distortion))),
                g.average_me_sad,
                g.total_gm_sbs,
                g.estimation_level,
                g.downsample_level,
                g.max_searches,
                u8::from(g.all_identity()),
            );
            eprintln!(
                "GMPORTMODELS poc={display_order} searched={} is_gm_on={}",
                // Did C's per-reference search actually RUN, or did the
                // frame-level derivation short-circuit it?
                u8::from(!g.all_identity() && gm_models.is_some()),
                gm_models.as_ref().map_or(2u8, |m| u8::from(m.is_gm_on)),
            );
            if let Some(m) = gm_models.as_ref() {
                for (l, row) in m.models.iter().enumerate() {
                    for (r, wm) in row.iter().enumerate() {
                        if wm.wm_type != svtav1_types::motion::TransformationType::Identity {
                            eprintln!(
                                "GMPORTREF poc={display_order} list={l} ref={r} wmtype={} \
                                     wmmat={:?} is_global={}",
                                wm.wm_type as u8,
                                wm.wmmat,
                                u8::from(m.is_global_motion[l][r]),
                            );
                        }
                    }
                }
            }
        }
    }
}

#[inline(always)]
pub(super) fn dump_refstats(
    display_order: u64,
    is_key: bool,
    frame_coded_area: &core::cell::RefCell<Option<CodedAreaAcc>>,
    coded_area_pct: (u8, u8, u8),
) {
    #[cfg(feature = "std")]
    if crate::dbgenv::refstats() {
        let (i_pct, s_pct, h_pct) = coded_area_pct;
        let acc = frame_coded_area.borrow();
        std::eprintln!(
            "PORTREFSTATS poc={display_order} slice={} intra={i_pct} skip={s_pct} hp={h_pct} \
                 mfmv={}/{} none={} sbintra=[{}] sbskip=[{}]",
            u8::from(is_key),
            // The MFMV writeback's census: cells naming a real reference,
            // out of the field's length. It is the positive control that
            // `av1_copy_frame_mvs` actually fired — a wire whose only
            // observable is a LATER frame the port still refuses would
            // otherwise be untestable. Zero on a key frame BY C'S GATE.
            acc.as_ref().map_or(0, |a| a
                .mvs
                .iter()
                .filter(|m| m.ref_frame > crate::port_coding_loop::INTRA_FRAME)
                .count()),
            acc.as_ref().map_or(0, |a| a.mvs.len()),
            // NONE cells: an intra block must reset its cells so a later
            // frame does not project stale motion through them.
            acc.as_ref().map_or(0, |a| a
                .mvs
                .iter()
                .filter(|m| m.ref_frame <= crate::port_coding_loop::INTRA_FRAME)
                .count()),
            acc.as_ref().map_or(alloc::string::String::new(), |a| a
                .sb_intra
                .iter()
                .map(|v| alloc::format!("{v}"))
                .collect::<alloc::vec::Vec<_>>()
                .join(",")),
            acc.as_ref().map_or(alloc::string::String::new(), |a| a
                .sb_skip
                .iter()
                .map(|v| alloc::format!("{v}"))
                .collect::<alloc::vec::Vec<_>>()
                .join(",")),
        );
    }
}

#[inline(always)]
pub(super) fn dump_mvs(ref_frame: &ReferenceFrame) {
    #[cfg(feature = "std")]
    if let Some(path) = std::env::var_os("SVTAV1_MVS_OUT") {
        // Diagnostic twin of the vendored-libaom `MVS` dump: the stored
        // per-8x8 motion field plus the saved ref_order_hint array, so a
        // field-by-field diff can separate "stored state diverged" from
        // "projection consumed it differently".
        use std::io::Write as _;
        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let mut w = std::io::BufWriter::new(file);
            let mut line = alloc::string::String::with_capacity(32 + ref_frame.mvs.len() * 14);
            core::fmt::Write::write_fmt(
                &mut line,
                format_args!("MVS {} {}", ref_frame.order_hint, ref_frame.mvs.len()),
            )
            .ok();
            for m in &ref_frame.mvs {
                core::fmt::Write::write_fmt(
                    &mut line,
                    format_args!(" {},{}", m.ref_frame, m.mv.as_int()),
                )
                .ok();
            }
            line.push('\n');
            core::fmt::Write::write_fmt(&mut line, format_args!("MVSROH {}", ref_frame.order_hint))
                .ok();
            for oh in ref_frame.ref_order_hint {
                core::fmt::Write::write_fmt(&mut line, format_args!(" {oh}")).ok();
            }
            line.push('\n');
            let _ = w.write_all(line.as_bytes());
        }
    }
}

impl EncodePipeline {
    #[inline(always)]
    pub(super) fn dump_recon10_bin(&self) {
        #[cfg(feature = "std")]
        if let Ok(prefix) = std::env::var("SVTAV1_RECON10_BIN") {
            if let (Some(y), Some((u, v))) =
                (self.last_recon10_y.as_ref(), self.last_recon10_uv.as_ref())
            {
                for (plane, samples) in [y, u, v].into_iter().enumerate() {
                    let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
                    std::fs::write(format!("{prefix}.p{plane}"), bytes)
                        .expect("write native pre-filter reconstruction");
                }
            }
        }
    }
}
