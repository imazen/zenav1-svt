//! CDF CONTINUATION — C's `FRAME_CONTEXT` as a saved, per-reference-slot
//! object, plus the frame-end counter reset that runs before it is saved.
//!
//! # Why this exists
//!
//! An AV1 frame header with `primary_ref_frame != PRIMARY_REF_NONE` and
//! `error_resilient_mode == 0` tells the decoder to start its tile CDFs from
//! the **end-of-frame state of the referenced frame**, not from the spec
//! defaults. The port's first inter frame header signals exactly that
//! (`primary_ref_frame = 0`, measured byte-identical to C — see
//! `docs/INTER-ENCODE-PLAN.md` §1r), so a tile coded against default CDFs
//! would not merely compress differently: it would **decode to garbage**. This
//! is a conformance requirement, not an RD detail.
//!
//! # The three C sites this module serves
//!
//! | C | what it does | port |
//! |---|---|---|
//! | `packetization_process.c:741-744` | `svt_av1_reset_cdf_symbol_counters(ec->fc)` then `((EbReferenceObject*)…)->frame_context = *ec->fc` | [`FrameCdfs::reset_symbol_counters`] + the store on [`crate::picture::ReferenceFrame`] |
//! | `ec_process.c:101-112` (`reset_entropy_coding_picture`) | copies `ref->frame_context` into the tile's `ec->fc` when `primary_ref_frame != PRIMARY_REF_NONE`, else `svt_aom_reset_entropy_coder` | the entropy walk's per-tile `FrameContext` / `CoeffFc` seed |
//! | `md_config_process.c:299-310` (`init_frame_rate_tables`) | copies the same into `pcs->md_frame_context`, else `svt_av1_default_coef_probs` + `svt_aom_init_mode_probs` | **NOT WIRED YET** — named in the module docs below |
//!
//! The `md_frame_context` half changes MODE DECISION rate estimates, not
//! syntax. It is deliberately left for the chunk that wires the inter branch of
//! MD, because until inter candidates exist there is nothing on an inter frame
//! whose cost it could change. Saying so here rather than silently omitting it.
//!
//! # Evidence
//!
//! Tier 2, and the oracle is direct rather than inferred: the C driver's
//! `__wrap_svt_av1_reset_cdf_symbol_counters`
//! (`tools/capture_c_trace/wrap_recon.c`) dumps the FRAME_CONTEXT **after** the
//! real reset and therefore reproduces byte-for-byte what lands in
//! `EbReferenceObject::frame_context`. `SVTAV1_FCTX_OUT` makes the port emit
//! the same field names in the same flat order, and
//! `tools/fctx_diff.py` compares them. That comparison does not need the inter
//! tile walk to work first, which is the whole point: the saved state can be
//! proven right before anything consumes it.
//!
//! # Two things this module does NOT carry, named rather than dropped
//!
//! * **`delta_lf_cdf` / `delta_lf_multi_cdf`.** C's `FRAME_CONTEXT` has them;
//!   [`crate::entropy::context::FrameContext`] does not, because nothing in
//!   this port ever sets `delta_lf_present`. They are therefore at their
//!   defaults on both sides for every frame this encoder can produce, and a
//!   saved state that omits them is indistinguishable from one that carries
//!   them — until `delta_lf_present` is implemented, at which point they must
//!   be added here in the same change.
//! * **The duplicated inter tables.** `FrameContext` carries `newmv_cdf`,
//!   `refmv_cdf`, `globalmv_cdf`, `drl_cdf`, `skip_mode_cdf`,
//!   `inter_compound_mode_cdf` and `interp_filter_cdf` as *placeholders*
//!   alongside the live copies in [`crate::port_entropy_inter::InterCdfs`].
//!   Both are seeded from the same tier-1 constants, so they agree at frame 0;
//!   the ones the inter writers adapt are `InterCdfs`', so **`InterCdfs` is
//!   what this module saves and restores**. That duplication is a live hazard
//!   for whoever wires the inter tile walk: adapt one copy and save the other
//!   and the stream desynchronises at the first mode symbol.
//!   [`tests::the_duplicated_inter_tables_agree_at_defaults`] pins the
//!   agreement so the drift cannot happen silently.

use crate::entropy::cdf::AomCdfProb;
use crate::entropy::coeff_c::CoeffFc;
use crate::entropy::context::FrameContext;
use crate::entropy::mv_coding::NmvContext;

/// C `FRAME_CONTEXT` (`cabac_context_model.h:278`) as one saved object.
///
/// The port splits the same state across two types — [`FrameContext`] for the
/// mode/partition/filter tables and [`CoeffFc`] for the coefficient and
/// ext-tx tables (which C keeps in the same struct). Saving them separately
/// would let a caller carry one and not the other; this type exists so that
/// cannot happen.
#[derive(Clone)]
pub struct FrameCdfs {
    /// The mode-side tables, including `inter` / `nmvc` / `ndvc`.
    pub fc: FrameContext,
    /// The coefficient + ext-tx tables. Boxed: ~26 KB.
    pub coeff: alloc::boxed::Box<CoeffFc>,
}

impl core::fmt::Debug for FrameCdfs {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // FrameContext/CoeffFc are ~40 KB of tables; a real Debug would be
        // unreadable AND would make `ReferenceFrame`'s derived Debug useless.
        f.write_str("FrameCdfs { .. }")
    }
}

/// C `reset_cdf_symbol_counter` (`cabac_context_model.c:1940`): zero the
/// adaptation counter at `[nsymbs]` of every CDF in a flat run of `num_cdfs`
/// arrays of stride `cdf_stride`.
///
/// The counter is NOT a probability. `update_cdf` reads it to pick the
/// adaptation RATE (`3 + (count > 15) + (count > 31) + …`), so a save that
/// skipped this would leave the next frame adapting at the slow, late-frame
/// rate from its very first symbol — a byte divergence with no visible cause
/// in any probability value.
#[inline]
fn reset_counter_stride(flat: &mut [AomCdfProb], cdf_stride: usize, nsymbs: usize) {
    debug_assert!(
        nsymbs < cdf_stride,
        "the counter lives at [nsymbs], inside the stride"
    );
    let num_cdfs = flat.len() / cdf_stride;
    for i in 0..num_cdfs {
        flat[i * cdf_stride + nsymbs] = 0;
    }
}

/// The common case: `cdf_stride == nsymbs + 1`, i.e. the counter is the last
/// element of each array. C's `RESET_CDF_COUNTER(x, n)` macro.
#[inline]
fn reset_counter(flat: &mut [AomCdfProb], nsymbs: usize) {
    reset_counter_stride(flat, nsymbs + 1, nsymbs);
}

fn reset_nmv_counter(nmv: &mut NmvContext) {
    reset_counter(&mut nmv.joints_cdf, 4);
    for c in &mut nmv.comps {
        reset_counter(&mut c.classes_cdf, 11);
        reset_counter(c.class0_fp_cdf.as_flattened_mut(), 4);
        reset_counter(&mut c.fp_cdf, 4);
        reset_counter(&mut c.sign_cdf, 2);
        reset_counter(&mut c.class0_hp_cdf, 2);
        reset_counter(&mut c.hp_cdf, 2);
        reset_counter(&mut c.class0_cdf, 2);
        reset_counter(c.bits_cdf.as_flattened_mut(), 2);
    }
}

impl FrameCdfs {
    /// C `svt_av1_reset_cdf_symbol_counters` (`cabac_context_model.c:1970`),
    /// in C's own order.
    ///
    /// Three of C's entries use a STRIDE wider than `nsymbs + 1`, and getting
    /// those wrong zeroes a probability instead of a counter:
    ///
    /// * `uv_mode_cdf[0]` — the CFL-disallowed plane has `UV_INTRA_MODES - 1`
    ///   symbols in an array sized for `UV_INTRA_MODES`;
    /// * `partition_cdf` — contexts 0..3 have 4 symbols and 16..19 have 8, both
    ///   in arrays sized for `EXT_PARTITION_TYPES` (10);
    /// * `tx_size_cdf[0]` — `MAX_TX_DEPTH` symbols in an array sized for
    ///   `MAX_TX_DEPTH + 1`; and the intra/inter ext-tx sets, whose per-set
    ///   symbol counts (7/5 and 16/12/2) are all under `TX_TYPES`.
    pub fn reset_symbol_counters(&mut self) {
        let c = &mut *self.coeff;
        reset_counter(c.txb_skip_cdf.as_flattened_mut(), 2);
        reset_counter(c.eob_extra_cdf.as_flattened_mut(), 2);
        reset_counter(c.dc_sign_cdf.as_flattened_mut(), 2);
        reset_counter(c.eob_flag_cdf16.as_flattened_mut(), 5);
        reset_counter(c.eob_flag_cdf32.as_flattened_mut(), 6);
        reset_counter(c.eob_flag_cdf64.as_flattened_mut(), 7);
        reset_counter(c.eob_flag_cdf128.as_flattened_mut(), 8);
        reset_counter(c.eob_flag_cdf256.as_flattened_mut(), 9);
        reset_counter(c.eob_flag_cdf512.as_flattened_mut(), 10);
        reset_counter(c.eob_flag_cdf1024.as_flattened_mut(), 11);
        reset_counter(c.coeff_base_eob_cdf.as_flattened_mut(), 3);
        reset_counter(c.coeff_base_cdf.as_flattened_mut(), 4);
        reset_counter(c.coeff_br_cdf.as_flattened_mut(), 4);

        let inter = &mut self.fc.inter;
        reset_counter(inter.newmv_cdf.as_flattened_mut(), 2);
        reset_counter(inter.zeromv_cdf.as_flattened_mut(), 2);
        reset_counter(inter.refmv_cdf.as_flattened_mut(), 2);
        reset_counter(inter.drl_cdf.as_flattened_mut(), 2);
        reset_counter(inter.inter_compound_mode_cdf.as_flattened_mut(), 8);
        reset_counter(inter.compound_type_cdf.as_flattened_mut(), 2);
        reset_counter(inter.wedge_idx_cdf.as_flattened_mut(), 16);
        reset_counter(inter.interintra_cdf.as_flattened_mut(), 2);
        reset_counter(inter.wedge_interintra_cdf.as_flattened_mut(), 2);
        reset_counter(inter.interintra_mode_cdf.as_flattened_mut(), 4);
        reset_counter(inter.motion_mode_cdf.as_flattened_mut(), 3);
        reset_counter(inter.obmc_cdf.as_flattened_mut(), 2);

        let fc = &mut self.fc;
        reset_counter(fc.palette_y_size_cdf.as_flattened_mut(), 7);
        // `palette_uv_size_cdf` is C-side only (this port never codes a UV
        // palette SIZE symbol through a stored CDF); see the module docs.
        for j in 0..7usize {
            // C: nsymbs = j + PALETTE_MIN_SIZE (2), stride CDF_SIZE(PALETTE_COLORS=8) = 9.
            reset_counter_stride(fc.palette_y_color_index_cdf[j].as_flattened_mut(), 9, j + 2);
        }
        reset_counter(
            fc.palette_y_mode_cdf.as_flattened_mut().as_flattened_mut(),
            2,
        );
        reset_counter(fc.palette_uv_mode_cdf.as_flattened_mut(), 2);
        reset_counter(fc.comp_inter_cdf.as_flattened_mut(), 2);
        reset_counter(fc.single_ref_cdf.as_flattened_mut().as_flattened_mut(), 2);
        reset_counter(self.fc.inter.comp_ref_type_cdf.as_flattened_mut(), 2);
        reset_counter(
            self.fc
                .inter
                .uni_comp_ref_cdf
                .as_flattened_mut()
                .as_flattened_mut(),
            2,
        );
        reset_counter(
            self.fc.comp_ref_cdf.as_flattened_mut().as_flattened_mut(),
            2,
        );
        reset_counter(
            self.fc
                .inter
                .comp_bwdref_cdf
                .as_flattened_mut()
                .as_flattened_mut(),
            2,
        );
        reset_counter(self.fc.txfm_partition_cdf.as_flattened_mut(), 2);
        reset_counter(self.fc.inter.compound_index_cdf.as_flattened_mut(), 2);
        reset_counter(self.fc.inter.comp_group_idx_cdf.as_flattened_mut(), 2);
        reset_counter(self.fc.inter.skip_mode_cdf.as_flattened_mut(), 2);
        reset_counter(self.fc.skip_cdf.as_flattened_mut(), 2);
        reset_counter(self.fc.intra_inter_cdf.as_flattened_mut(), 2);
        reset_nmv_counter(&mut self.fc.nmvc);
        reset_nmv_counter(&mut self.fc.ndvc);
        reset_counter(&mut self.fc.intrabc_cdf, 2);
        reset_counter(&mut self.fc.seg_tree_cdf, 8);
        reset_counter(self.fc.seg_pred_cdf.as_flattened_mut(), 2);
        reset_counter(self.fc.spatial_pred_seg_cdf.as_flattened_mut(), 8);
        reset_counter(self.fc.filter_intra_cdfs.as_flattened_mut(), 2);
        reset_counter(&mut self.fc.filter_intra_mode_cdf, 5);
        reset_counter(&mut self.fc.switchable_restore_cdf, 3);
        reset_counter(&mut self.fc.wiener_restore_cdf, 2);
        reset_counter(&mut self.fc.sgrproj_restore_cdf, 2);
        reset_counter(self.fc.y_mode_cdf.as_flattened_mut(), 13);
        // C: uv_mode_cdf[0] has UV_INTRA_MODES - 1 = 13 symbols in a stride of
        // CDF_SIZE(UV_INTRA_MODES) = 15; uv_mode_cdf[1] has 14 in the same 15.
        reset_counter_stride(self.fc.uv_mode_cdf[0].as_flattened_mut(), 15, 13);
        reset_counter(self.fc.uv_mode_cdf[1].as_flattened_mut(), 14);
        for i in 0..self.fc.partition_cdf.len() {
            let nsymbs = if i < 4 {
                4
            } else if i < 16 {
                10
            } else {
                8
            };
            reset_counter_stride(&mut self.fc.partition_cdf[i], 11, nsymbs);
        }
        reset_counter(self.fc.inter.switchable_interp_cdf.as_flattened_mut(), 3);
        reset_counter(
            self.fc.kf_y_mode_cdf.as_flattened_mut().as_flattened_mut(),
            13,
        );
        reset_counter(self.fc.angle_delta_cdf.as_flattened_mut(), 7);
        // tx_size_cdf[0]: MAX_TX_DEPTH (2) symbols, stride CDF_SIZE(3) = 4.
        reset_counter_stride(self.fc.tx_size_cdf[0].as_flattened_mut(), 4, 2);
        for cat in 1..self.fc.tx_size_cdf.len() {
            reset_counter(self.fc.tx_size_cdf[cat].as_flattened_mut(), 3);
        }
        reset_counter(&mut self.fc.delta_q_cdf, 4);
        // delta_lf_cdf / delta_lf_multi_cdf: see the module docs — absent from
        // this port's FrameContext because delta_lf_present is never signalled.
        let c = &mut *self.coeff;
        // C `intra_ext_tx_cdf[set]`: set 1 has 7 symbols, set 2 has 5, both in
        // a stride of CDF_SIZE(TX_TYPES) = 17. Set 0 is never coded.
        // The port stores all three sets in one flat [3][4][13] run of 17s.
        for (set, nsymbs) in [(1usize, 7usize), (2, 5)] {
            let base = set * 4 * 13;
            reset_counter_stride(
                c.intra_ext_tx_cdf[base..base + 4 * 13].as_flattened_mut(),
                17,
                nsymbs,
            );
        }
        for (set, nsymbs) in [(1usize, 16usize), (2, 12), (3, 2)] {
            let base = set * 4;
            reset_counter_stride(
                c.inter_ext_tx_cdf[base..base + 4].as_flattened_mut(),
                17,
                nsymbs,
            );
        }
        reset_counter(&mut self.fc.cfl_sign_cdf, 8);
        reset_counter(self.fc.cfl_alpha_cdf.as_flattened_mut(), 16);
    }

    /// Visit every stored CDF run under the C `FRAME_CONTEXT` field name the
    /// oracle dump uses, in C's declaration order.
    ///
    /// **This is the single enumeration.** The read-only [`Self::for_each_field`]
    /// is derived from it rather than repeating it, because two lists of sixty
    /// fields are two lists that can disagree — and the way they would disagree
    /// (a field present in the dump but missing from the save, or vice versa)
    /// is invisible in any pass/fail comparison.
    pub fn for_each_field_mut(&mut self, f: &mut dyn FnMut(&str, &mut [AomCdfProb])) {
        let c = &mut *self.coeff;
        f("txb_skip", c.txb_skip_cdf.as_flattened_mut());
        f("eob_extra", c.eob_extra_cdf.as_flattened_mut());
        f("dc_sign", c.dc_sign_cdf.as_flattened_mut());
        f("eob_flag16", c.eob_flag_cdf16.as_flattened_mut());
        f("eob_flag32", c.eob_flag_cdf32.as_flattened_mut());
        f("eob_flag64", c.eob_flag_cdf64.as_flattened_mut());
        f("eob_flag128", c.eob_flag_cdf128.as_flattened_mut());
        f("eob_flag256", c.eob_flag_cdf256.as_flattened_mut());
        f("eob_flag512", c.eob_flag_cdf512.as_flattened_mut());
        f("eob_flag1024", c.eob_flag_cdf1024.as_flattened_mut());
        f("coeff_base_eob", c.coeff_base_eob_cdf.as_flattened_mut());
        f("coeff_base", c.coeff_base_cdf.as_flattened_mut());
        f("coeff_br", c.coeff_br_cdf.as_flattened_mut());
        {
            let i = &mut self.fc.inter;
            f("newmv", i.newmv_cdf.as_flattened_mut());
            f("zeromv", i.zeromv_cdf.as_flattened_mut());
            f("refmv", i.refmv_cdf.as_flattened_mut());
            f("drl", i.drl_cdf.as_flattened_mut());
            f(
                "inter_compound_mode",
                i.inter_compound_mode_cdf.as_flattened_mut(),
            );
            f("compound_type", i.compound_type_cdf.as_flattened_mut());
            f("wedge_idx", i.wedge_idx_cdf.as_flattened_mut());
            f("interintra", i.interintra_cdf.as_flattened_mut());
            f(
                "wedge_interintra",
                i.wedge_interintra_cdf.as_flattened_mut(),
            );
            f("interintra_mode", i.interintra_mode_cdf.as_flattened_mut());
            f("motion_mode", i.motion_mode_cdf.as_flattened_mut());
            f("obmc", i.obmc_cdf.as_flattened_mut());
        }
        let fc = &mut self.fc;
        f("palette_y_size", fc.palette_y_size_cdf.as_flattened_mut());
        f(
            "palette_y_color_index",
            fc.palette_y_color_index_cdf
                .as_flattened_mut()
                .as_flattened_mut(),
        );
        f(
            "palette_y_mode",
            fc.palette_y_mode_cdf.as_flattened_mut().as_flattened_mut(),
        );
        f("palette_uv_mode", fc.palette_uv_mode_cdf.as_flattened_mut());
        f("comp_inter", fc.comp_inter_cdf.as_flattened_mut());
        f(
            "single_ref",
            fc.single_ref_cdf.as_flattened_mut().as_flattened_mut(),
        );
        f(
            "comp_ref_type",
            fc.inter.comp_ref_type_cdf.as_flattened_mut(),
        );
        f(
            "uni_comp_ref",
            fc.inter
                .uni_comp_ref_cdf
                .as_flattened_mut()
                .as_flattened_mut(),
        );
        f(
            "comp_ref",
            fc.comp_ref_cdf.as_flattened_mut().as_flattened_mut(),
        );
        f(
            "comp_bwdref",
            fc.inter
                .comp_bwdref_cdf
                .as_flattened_mut()
                .as_flattened_mut(),
        );
        f("txfm_partition", fc.txfm_partition_cdf.as_flattened_mut());
        f(
            "compound_index",
            fc.inter.compound_index_cdf.as_flattened_mut(),
        );
        f(
            "comp_group_idx",
            fc.inter.comp_group_idx_cdf.as_flattened_mut(),
        );
        f("skip_mode", fc.inter.skip_mode_cdf.as_flattened_mut());
        f("skip", fc.skip_cdf.as_flattened_mut());
        f("intra_inter", fc.intra_inter_cdf.as_flattened_mut());
        nmv_fields("nmvc", &mut fc.nmvc, f);
        nmv_fields("ndvc", &mut fc.ndvc, f);
        f("intrabc", &mut fc.intrabc_cdf);
        f("seg.tree", &mut fc.seg_tree_cdf);
        f("seg.pred", fc.seg_pred_cdf.as_flattened_mut());
        f(
            "seg.spatial_pred",
            fc.spatial_pred_seg_cdf.as_flattened_mut(),
        );
        f("filter_intra", fc.filter_intra_cdfs.as_flattened_mut());
        f("filter_intra_mode", &mut fc.filter_intra_mode_cdf);
        f("switchable_restore", &mut fc.switchable_restore_cdf);
        f("wiener_restore", &mut fc.wiener_restore_cdf);
        f("sgrproj_restore", &mut fc.sgrproj_restore_cdf);
        f("y_mode", fc.y_mode_cdf.as_flattened_mut());
        f(
            "uv_mode",
            fc.uv_mode_cdf.as_flattened_mut().as_flattened_mut(),
        );
        f("partition", fc.partition_cdf.as_flattened_mut());
        f(
            "switchable_interp",
            fc.inter.switchable_interp_cdf.as_flattened_mut(),
        );
        f(
            "kf_y",
            fc.kf_y_mode_cdf.as_flattened_mut().as_flattened_mut(),
        );
        f("angle_delta", fc.angle_delta_cdf.as_flattened_mut());
        f(
            "tx_size",
            fc.tx_size_cdf.as_flattened_mut().as_flattened_mut(),
        );
        f("delta_q", &mut fc.delta_q_cdf);
        f("intra_ext_tx", c.intra_ext_tx_cdf.as_flattened_mut());
        f("inter_ext_tx", c.inter_ext_tx_cdf.as_flattened_mut());
        f("cfl_sign", &mut fc.cfl_sign_cdf);
        f("cfl_alpha", fc.cfl_alpha_cdf.as_flattened_mut());
    }

    /// Read-only view of [`Self::for_each_field_mut`]. Clones (~40 KB) so the
    /// one enumeration can stay `&mut`; only dump/test paths call it.
    pub fn for_each_field(&self, f: &mut dyn FnMut(&str, &[AomCdfProb])) {
        let mut tmp = self.clone();
        tmp.for_each_field_mut(&mut |name, vals| f(name, vals));
    }

    /// `SVTAV1_FCTX_OUT` — append this state in the C oracle's format:
    /// `FCTX <call> <field> <count> <v0> <v1> …`.
    ///
    /// Appends, like every other dump in this repo, so truncate before a run
    /// (`tools/fctx_gate.sh` does).
    #[cfg(feature = "std")]
    pub fn dump_to(&self, path: &std::ffi::OsStr, call: u32) {
        use std::io::Write as _;
        let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        else {
            return;
        };
        let mut w = std::io::BufWriter::new(file);
        let mut err = false;
        self.for_each_field(&mut |name, vals| {
            if err {
                return;
            }
            let mut line = alloc::string::String::with_capacity(vals.len() * 6 + 32);
            core::fmt::Write::write_fmt(
                &mut line,
                format_args!("FCTX {call} {name} {}", vals.len()),
            )
            .ok();
            for v in vals {
                core::fmt::Write::write_fmt(&mut line, format_args!(" {v}")).ok();
            }
            line.push('\n');
            err = w.write_all(line.as_bytes()).is_err();
        });
        let _ = w.flush();
    }
}

fn nmv_fields(prefix: &str, nmv: &mut NmvContext, f: &mut dyn FnMut(&str, &mut [AomCdfProb])) {
    f(&alloc::format!("{prefix}.joints"), &mut nmv.joints_cdf);
    for (idx, c) in nmv.comps.iter_mut().enumerate() {
        let p = alloc::format!("{prefix}.comp{idx}");
        f(&alloc::format!("{p}.classes"), &mut c.classes_cdf);
        f(
            &alloc::format!("{p}.class0_fp"),
            c.class0_fp_cdf.as_flattened_mut(),
        );
        f(&alloc::format!("{p}.fp"), &mut c.fp_cdf);
        f(&alloc::format!("{p}.sign"), &mut c.sign_cdf);
        f(&alloc::format!("{p}.class0_hp"), &mut c.class0_hp_cdf);
        f(&alloc::format!("{p}.hp"), &mut c.hp_cdf);
        f(&alloc::format!("{p}.class0"), &mut c.class0_cdf);
        f(&alloc::format!("{p}.bits"), c.bits_cdf.as_flattened_mut());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> FrameCdfs {
        FrameCdfs {
            fc: FrameContext::new_default(),
            coeff: CoeffFc::default_for_qindex(40),
        }
    }

    /// POSITIVE CONTROL for the counter reset: it must actually find and clear
    /// something. Fill every stored element with a marker, reset, and require
    /// that (a) at least one element per field became 0 and (b) the elements
    /// that are NOT counters kept the marker. Without (b) a reset that zeroed
    /// whole arrays would pass (a).
    #[test]
    fn the_counter_reset_clears_counters_and_nothing_else() {
        const MARK: AomCdfProb = 0x1234;
        let mut painted = fresh();
        painted.for_each_field_mut(&mut |_, v| {
            for x in v.iter_mut() {
                *x = MARK;
            }
        });
        let mut reset = painted.clone();
        reset.reset_symbol_counters();

        let mut before: alloc::vec::Vec<(alloc::string::String, usize)> = alloc::vec::Vec::new();
        painted.for_each_field(&mut |n, v| before.push((n.into(), v.len())));
        let mut cleared_total = 0usize;
        let mut kept_total = 0usize;
        let mut idx = 0usize;
        reset.for_each_field(&mut |n, v| {
            let cleared = v.iter().filter(|x| **x == 0).count();
            let kept = v.iter().filter(|x| **x == MARK).count();
            assert_eq!(
                cleared + kept,
                v.len(),
                "{n}: every element is either a cleared counter or the untouched marker"
            );
            assert!(
                cleared > 0,
                "{n}: the reset touched nothing — is it in C's list?"
            );
            assert_eq!(before[idx].1, v.len(), "{n}: length changed");
            cleared_total += cleared;
            kept_total += kept;
            idx += 1;
        });
        assert!(cleared_total > 0 && kept_total > 0);
    }

    #[test]
    fn the_duplicated_inter_tables_agree_at_defaults() {
        let fc = FrameContext::new_default();
        assert_eq!(fc.newmv_cdf, fc.inter.newmv_cdf, "newmv");
        assert_eq!(fc.refmv_cdf, fc.inter.refmv_cdf, "refmv");
        assert_eq!(fc.globalmv_cdf, fc.inter.zeromv_cdf, "globalmv/zeromv");
        assert_eq!(fc.drl_cdf, fc.inter.drl_cdf, "drl");
        assert_eq!(fc.skip_mode_cdf, fc.inter.skip_mode_cdf, "skip_mode");
        assert_eq!(
            fc.inter_compound_mode_cdf, fc.inter.inter_compound_mode_cdf,
            "inter_compound_mode"
        );
        assert_eq!(
            fc.interp_filter_cdf, fc.inter.switchable_interp_cdf,
            "interp_filter/switchable_interp"
        );
    }
}
