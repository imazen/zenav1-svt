use super::*;

/// Environment for the refinement gates: the chained rate tables supply
/// the ctx-0 PARTITION_SPLIT rates (`svt_aom_partition_rate_cost(.., 0,
/// 0)` — C passes zero partition contexts here, enc_dec_process.c:1585 /
/// :1613 / :1764).
pub(super) struct RefineEnv<'a> {
    pub(super) ctrls: &'a DrCtrls,
    /// C `ctx->disallow_4x4` for THIS superblock — `pic_disallow_4x4` as
    /// `set_depth_removal_level_controls` left it (it can only be SET,
    /// never cleared; enc_mode_config.c:3270-3278).
    pub(super) disallow_4x4: bool,
    /// C `ctx->disallow_8x8` — `get_disallow_8x8_{default,allintra}` are
    /// false on every reachable arm; carried for the clamp's shape.
    pub(super) disallow_8x8: bool,
    /// C `ctx->depth_removal_ctrls` for THIS superblock. Zeroed
    /// (`enabled = 0`) on an I-slice — `set_depth_removal_level_controls`
    /// returns early there (enc_mode_config.c:2973).
    pub(super) depth_removal: crate::port_enc_mode_config::common::DepthRemovalCtrls,
    pub(super) lambda: u64,
    pub(super) tables: &'a M6Pd0Tables,
    pub(super) max_pd0: usize,
    pub(super) min_pd0: usize,
    /// C `max_sq_size` (enc_dec_process.c:1814-1817):
    /// `ctx->max_block_size`, then `MIN(.., 32)` when
    /// `static_config.max_tx_size == 32`.
    ///
    /// This used to be the literal 64. The port ALREADY derives
    /// `max_tx_size = 32` at tune IQ with qp <= 45 (`hdr_mode.rs`) and threads
    /// it into every PD0 entry, so hardcoding 64 here admitted a shallower
    /// depth than C tests: at `--tune 3`, qp <= 45, presets 0-5 a 32x32 node
    /// got `s = -1` where C gives 0, and a 16x16 node `s = -2` where C gives
    /// -1.
    pub(super) max_sq: usize,
    /// `pcs->scs->super_block_size` — the `sq_size == 64 && sb == 64` (or
    /// 128) gate in `update_pred_th_offset`'s `use_ref_info` arm
    /// (enc_dec_process.c:1623-1624). Only the SB root can satisfy it.
    pub(super) sb_sq: usize,
    /// C `ref_obj_l0->sb_min_sq_size[sb_index]` / `sb_max_sq_size[sb_index]`
    /// for the use_ref_info arm — `Some` exactly when C's
    /// `slice_type != I_SLICE && svt_aom_is_ref_same_size(L0)` holds
    /// (:1608-1611). The B-slice `ref_list1` fold (:1617-1621) is inert on
    /// this port's low-delay envelope (`ref_list1_count_try == 0`).
    pub(super) ref_min_max_sq: Option<(u8, u8)>,
    /// `pcs->slice_type == I_SLICE` for the `coeff_lvl_modulation` gate
    /// (:1866). A video-mode I-slice (key frame) counts as I-slice here,
    /// same as the `use_ref_info` arm's `slice_type != I_SLICE` predicate.
    pub(super) is_islice: bool,
    /// `pcs->coeff_lvl` (`derive_inter_coeff_level`) for the same gate —
    /// the clamp engages only at NORMAL_LVL / HIGH_LVL. On an I-slice the
    /// value is inert (the `is_islice` test short-circuits first), so the
    /// caller may pass `Normal` there.
    pub(super) coeff_lvl: crate::quant::CoeffLvl,
}

/// C `update_pred_th_offset` (enc_dec_process.c:1545) + the deviation
/// gates, producing this PD0 leaf's admitted (s_depth, e_depth).
/// `parent` is the enclosing square's PD0 eval (None only for the SB
/// root, whose s is forced 0 by the max-size clamp anyway).
// `abs_x`/`abs_y` are consumed only by the std-gated NSQDBG REFINE dump below.
#[cfg_attr(not(feature = "std"), allow(unused_variables))]
pub(super) fn set_start_end_depth(
    env: &RefineEnv<'_>,
    node: &Pd0Eval,
    parent: Option<&Pd0Eval>,
    abs_x: usize,
    abs_y: usize,
) -> (i32, i32) {
    let ctrls = env.ctrls;
    if !ctrls.adaptive {
        return (0, 0);
    }
    let sq = node.sq;
    let mut s: i32 = -2;
    let mut e: i32 = 2;
    // 4x4 has no children (set_start_end_depth, enc_dec_process.c:1784).
    if sq == 4 {
        e = 0;
    }
    // The disallow_8x8 arm REPLACES disallow_4x4's (:1788-1797); the
    // per-SB depth-removal flags then clamp on top of either (:1799-1813).
    if env.disallow_8x8 {
        e = if sq <= 16 {
            0
        } else if sq == 32 {
            e.min(1)
        } else if sq == 64 {
            e.min(2)
        } else if sq == 128 {
            e.min(3)
        } else {
            e
        };
    } else if env.disallow_4x4 {
        e = match sq {
            8 => 0,
            16 => e.min(1),
            32 => e.min(2),
            _ => e,
        };
    }
    if env.depth_removal.enabled != 0 {
        if env.depth_removal.disallow_below_64x64 != 0 {
            e = if sq <= 64 {
                0
            } else if sq == 128 {
                e.min(1)
            } else {
                e
            };
        } else if env.depth_removal.disallow_below_32x32 != 0 {
            e = if sq <= 32 {
                0
            } else if sq == 64 {
                e.min(1)
            } else if sq == 128 {
                e.min(2)
            } else {
                e
            };
        } else if env.depth_removal.disallow_below_16x16 != 0 {
            e = if sq <= 16 {
                0
            } else if sq == 32 {
                e.min(1)
            } else if sq == 64 {
                e.min(2)
            } else if sq == 128 {
                e.min(3)
            } else {
                e
            };
        }
    }
    // C :1819-1823, against the real `max_sq_size` (see `RefineEnv::max_sq`).
    if sq == env.max_sq {
        s = 0;
    } else if s == -2 && sq * 2 == env.max_sq {
        s = -1;
    }

    let mut add_parent = true;
    let mut add_sub = true;
    // C `mode == PD0_DEPTH_ADAPTIVE && (s_depth != 0 || e_depth != 0)`
    // (enc_dec_process.c:1826). NO_RESTRICTION reaches here with the seeded
    // -2/+2 (clamped above) and skips the whole narrowing.
    if !ctrls.no_restriction && (s != 0 || e != 0) {
        add_parent = false;
        add_sub = false;

        // limit_max_min_to_pd0 (:1846-1863).
        if ctrls.limit_to_pd0 != 0 && env.max_pd0 / env.min_pd0 > ctrls.limit_to_pd0 {
            if sq == env.max_pd0 {
                s = 0;
            }
            if sq == env.min_pd0 {
                e = 0;
            }
            if s == -2 && sq * 2 == env.max_pd0 {
                s = -1;
            }
            if e == 2 && sq / 2 == env.min_pd0 {
                e = 1;
            }
        }
        // coeff_lvl_modulation (:1865-1870): dead on I-slices; on an inter
        // frame at NORMAL/HIGH `coeff_lvl` it narrows the raw -2/+2 span to
        // one level each way. Without it the PD1 walk tests a grandparent
        // shape C never admits — an 8x8-leaf PD0 tree would grow a 32x16
        // candidate at the 32x32 node where C stops at the 16x16 parent.
        if ctrls.coeff_lvl_mod
            && !env.is_islice
            && !matches!(
                env.coeff_lvl,
                crate::quant::CoeffLvl::VLow | crate::quant::CoeffLvl::Low
            )
        {
            s = s.max(-1);
            e = e.min(1);
        }

        let mut s_off: i64 = 0;
        let mut e_off: i64 = 0;
        // update_pred_th_offset (:1545): cost-band modulation (M5 only).
        if ctrls.band_mod {
            let max_cost = rdcost(env.lambda, 16, ctrls.max_cost_multiplier * (sq * sq) as u64);
            // C `update_pred_th_offset` (enc_dec_process.c:1550) guards on
            // `tested_blk[PART_N][0]`: an incomplete block whose PART_N was
            // never costed has no SQ cost to band.
            if node.sq_tested && node.cost <= max_cost {
                let band_size = max_cost / ctrls.max_band_cnt;
                let band_idx = (node.cost / band_size) as usize;
                // cost == max_cost lands on band_idx == max_band_cnt; the
                // C ctrls array has no such slot (uninitialized read of a
                // zeroed struct field in practice) — treat as offset 0.
                if band_idx < 4 {
                    if ctrls.decrement_per_band[band_idx] == i64::MAX {
                        s = 0;
                        e = 0;
                    } else {
                        s_off = -ctrls.decrement_per_band[band_idx];
                        e_off = -ctrls.decrement_per_band[band_idx];
                    }
                }
            }
        }
        // lower_depth_split_cost_th (:1573-1592): drop the parent depth
        // when splitting the PARENT is very cheap relative to its cost.
        if s != 0
            && ctrls.lower_split_th != 0
            && let Some(p) = parent
        {
            // C :1566 `pc_tree->parent->tested_blk[PART_N][0]`.
            if p.sq_tested {
                let split_cost = rdcost(env.lambda, env.tables.split_bits(p.sq), 0);
                if split_cost * 10000 < p.cost * ctrls.lower_split_th {
                    s = 0;
                }
            }
        }
        // split_rate_th (+20, CLN_PD0 :1594-1619): drop the child depth
        // when splitting THIS block is expensive relative to its cost.
        // C :1586 `split_cost_th && pc_tree->tested_blk[PART_N][0]`.
        if ctrls.split_rate_th != 0 && node.sq_tested {
            let th = ctrls.split_rate_th + 20;
            let split_cost = rdcost(env.lambda, env.tables.split_bits(sq), 0);
            if split_cost * 1000 > node.cost * th {
                e = 0;
            }
        }
        // use_ref_info (:1606-1631): on an inter frame with a same-size L0
        // reference, an SB-sized node whose co-located reference SB coded a
        // uniform square takes s = e = 0 — no sub-depths, no parent depth.
        if ctrls.use_ref_info
            && sq == env.sb_sq
            && let Some((mn, mx)) = env.ref_min_max_sq
            && usize::from(mn) == sq
            && usize::from(mx) == sq
        {
            s = 0;
            e = 0;
        }

        // is_parent_to_current_deviation_small (:1650): only called for
        // tested blocks below the SB size (:1876-1883).
        // C :1859-1861 — "Check tested_blk b/c use block's cost inside".
        if s != 0 && node.sq_tested && sq < 64 {
            // C `is_parent_to_current_deviation_small`'s own
            // `pc_tree->parent->tested_blk[PART_N][0]` guard (:1634).
            match parent.filter(|p| p.sq_tested) {
                Some(p) => {
                    // s1 used RAW + offset (the qp-scaling is disabled:
                    // depths_qp_based_th_scaling = 0 for allintra <= M6);
                    // s2 = 255 -> MIN_SIGNED (always passes).
                    let s1_th = ctrls.s1_th + s_off;
                    let max_cost = if ctrls.parent_max_cost_mult != 0 {
                        rdcost(
                            env.lambda,
                            18000 * ctrls.parent_max_cost_mult,
                            60 * ctrls.parent_max_cost_mult * (sq * sq) as u64 * 4,
                        )
                    } else {
                        0
                    };
                    let cur4 = (node.cost * 4).max(1) as i64;
                    let dev = ((p.cost.max(1) as i64) - cur4) * 100 / cur4;
                    if dev >= s1_th && p.cost >= max_cost {
                        s = 0;
                    } else if dev >= ctrls.s2_th {
                        // s2 = MIN_SIGNED sentinel (levels 5/6/9) -> always
                        // here; s2 = literal 0 (levels 1-4) -> here iff dev>=0.
                        s = -1;
                    } else {
                        // C `MAX(*s_depth, -2)` (:1697): a negative parent
                        // deviation admits the grandparent depth too.
                        s = s.max(-2);
                    }
                }
                None => {
                    // pd0_unavail_mode_depth (:1700-1706): 0 -> s = 0;
                    // 1 -> s = max(s, -1); 2 -> unchanged.
                    match ctrls.unavail_mode {
                        0 => s = 0,
                        1 => s = s.max(-1),
                        _ => {}
                    }
                }
            }
            if s != 0 {
                add_parent = true;
            }
        }

        // is_child_to_current_deviation_small (:1709): gated on tested +
        // sq > 4 (:1885-1892).
        // C :1868-1870 — the same tested_blk guard on the child arm.
        if e != 0 && node.sq_tested && sq > 4 {
            let tested_children: Vec<&Pd0Eval> = node
                .children
                .as_ref()
                // C `is_child_to_current_deviation_small` (:1697-1712) sums
                // `split[i]->block_data[PART_N][0]->cost` only for children
                // whose `tested_blk[PART_N][0]` is set — a boundary child
                // contributes neither cost nor count.
                .map(|ch| ch.iter().filter(|c| c.sq_tested).collect())
                .unwrap_or_default();
            if !tested_children.is_empty() {
                // e1 qp-scaled with factors 1/1 (scaling disabled) + off;
                // e2 = 255 -> MIN_SIGNED.
                let e1_th = ctrls.e1_th + e_off;
                let sum: u64 = tested_children.iter().map(|c| c.cost).sum();
                let mut child_cost = (sum / tested_children.len() as u64) * 4;
                child_cost += rdcost(env.lambda, env.tables.split_bits(sq), 0);
                let cur = node.cost.max(1) as i64;
                let dev = ((child_cost.max(1) as i64) - cur) * 100 / cur;
                if dev >= e1_th {
                    e = 0;
                } else if dev >= ctrls.e2_th {
                    // e2 = MIN_SIGNED sentinel (levels 5/6/9) -> always here;
                    // e2 = literal 0 (levels 1-4) -> here iff dev>=0.
                    e = 1;
                } else {
                    // C `MIN(*e_depth, 2)` (:1729): a negative child deviation
                    // admits the grandchild depth too.
                    e = e.min(2);
                }
            } else {
                match ctrls.unavail_mode {
                    0 => e = 0,
                    1 => e = e.min(1),
                    _ => {}
                }
            }
            if e != 0 {
                add_sub = true;
            }
        }
    }

    #[cfg(feature = "std")]
    if nsqdbg_here(abs_x, abs_y) {
        let ch_costs: Vec<u64> = node
            .children
            .as_ref()
            .map(|ch| ch.iter().filter(|c| c.sq_tested).map(|c| c.cost).collect())
            .unwrap_or_default();
        eprintln!(
            "NSQDBG REFINE mi=({},{}) sq={} tested={} cost={} pcost={} maxpd0={} minpd0={} sb={} psb={} ch={:?} s={} e={}",
            abs_y / 4,
            abs_x / 4,
            sq,
            u8::from(node.sq_tested),
            node.cost,
            parent.map(|p| p.cost as i64).unwrap_or(-1),
            env.max_pd0,
            env.min_pd0,
            env.tables.split_bits(sq),
            parent
                .map(|p| env.tables.split_bits(p.sq) as i64)
                .unwrap_or(-1),
            ch_costs,
            if add_parent { s } else { 0 },
            if add_sub { e } else { 0 },
        );
    }
    (if add_parent { s } else { 0 }, if add_sub { e } else { 0 })
}

/// C `refine_depth` (enc_dec_process.c:1901): walk the PD0 pc_tree and
/// build the refined MdScan marks. Returns the subtree's s_depth
/// propagation (parent-depth admissions bubble up: a SPLIT node whose
/// children admit their parent evaluates ITS PART_N, :1947-1953).
pub(super) fn refine_depth(
    env: &RefineEnv<'_>,
    node: &Pd0Eval,
    parent: Option<&Pd0Eval>,
    abs_x: usize,
    abs_y: usize,
) -> (RefScan, i32) {
    let mut scan = RefScan::leaf(node.sq);
    if !node.split {
        scan.test_this = true;
        let (s, e) = set_start_end_depth(env, node, parent, abs_x, abs_y);
        if e > 0 {
            scan.set_children_tested(e, env.disallow_4x4, env.disallow_8x8);
        }
        (scan, s)
    } else {
        let ch_evals = node.children.as_ref().expect("split children");
        let mut s_min = 0i32;
        let half = node.sq / 2;
        let mut ch: [RefScan; 4] = [
            RefScan::leaf(half),
            RefScan::leaf(half),
            RefScan::leaf(half),
            RefScan::leaf(half),
        ];
        for (i, cev) in ch_evals.iter().enumerate() {
            let (cs, s_child) = refine_depth(
                env,
                cev,
                Some(node),
                abs_x + (i & 1) * half,
                abs_y + (i >> 1) * half,
            );
            ch[i] = cs;
            s_min = s_min.min(s_child);
        }
        scan.split_flag = true;
        scan.children = Some(Box::new(ch));
        let mut s = s_min;
        // I-slice: blocks < 128 allowed (:1946).
        if s < 0 && node.sq < 128 {
            scan.test_this = true;
            s += 1;
        }
        (scan, s)
    }
}

/// C `perform_pred_depth_refinement` (enc_dec_process.c:1985).
// Production uses `build_refined_scan_at`; the C-shaped wrapper is also
// exercised directly by the capture tests in this module.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn build_refined_scan(
    root: &Pd0Eval,
    ctrls: &DrCtrls,
    lambda: u64,
    tables: &M6Pd0Tables,
) -> RefScan {
    // `None` = derive the PD0 max/min from THIS root alone. Correct whenever
    // the root spans a whole superblock — i.e. every SB64 case and the tests
    // below. The SB128 pipeline passes the whole-128-SB fold instead (see
    // `build_refined_scan_at`).
    build_refined_scan_at(
        root,
        ctrls,
        lambda,
        tables,
        0,
        0,
        None,
        64,
        64,
        None,
        true,
        crate::quant::CoeffLvl::Normal,
        // Test-side env: `ctrls.disallow_4x4` stands in for the resolved
        // `ctx->disallow_4x4` and the depth-removal controls stay disabled —
        // the key-frame arm, where `set_depth_removal_level_controls`
        // returns early.
        ctrls.disallow_4x4,
        false,
        crate::port_enc_mode_config::common::DepthRemovalCtrls::default(),
    )
}

/// [`build_refined_scan`] with the SB's pixel origin, so the NSQDBG REFINE
/// dump (gated by SVTAV1_DBG_MI) can label nodes with absolute mi coords.
pub(crate) fn build_refined_scan_at(
    root: &Pd0Eval,
    ctrls: &DrCtrls,
    lambda: u64,
    tables: &M6Pd0Tables,
    sb_x: usize,
    sb_y: usize,
    // Whole-superblock PD0 (max, min) block sizes. `None` = derive from
    // `root` alone (the single-64x64-unit case: SB64, or a partial SB128
    // unit). `Some` carries C's WHOLE-128-SB fold for the SB128 refined
    // path: C's `get_max_min_pd0_depths` (enc_dec_process.c:1943) walks the
    // ENTIRE SB pc_tree — at SB128 that is all four 64x64 coding-unit
    // quadrants — so `max_pd0_size`/`min_pd0_size` fed to `set_start_end_depth`
    // span the whole 128 SB, NOT this one 64x64 unit. Computing them
    // per-unit made a quadrant whose PD0 max was 16 (while a sibling quadrant
    // reached 32) cap its shallowest tested depth at 16x16, force-splitting
    // the 32x32 nodes C keeps (`limit_max_min_to_pd0`, :1830-1846). Only bit
    // at SB128 where units.len() > 1; at SB64 the fold equals the root's own
    // max/min, so passing it is byte-identical.
    sb_max_min: Option<(usize, usize)>,
    // Effective square cap: 32/64 from max_tx_size, or 8 for lossless.
    // The lossless cap folds in init_md_scan's final geometry filter;
    // its NO_RESTRICTION mode does not read the deviation heuristics.
    max_tx_size: u8,
    // `pcs->scs->super_block_size` for the use_ref_info arm's
    // `sq_size == sb_size` gate (enc_dec_process.c:1623).
    sb_sq: usize,
    // `(ref_obj_l0->sb_min_sq_size[sb], sb_max_sq_size[sb])` for THIS
    // superblock — `None` on a key frame / no same-size L0 reference, which
    // is C's `slice_type == I_SLICE || !is_ref_l0_avail` arm.
    ref_min_max_sq: Option<(u8, u8)>,
    // C `pcs->slice_type == I_SLICE` for the `coeff_lvl_modulation` gate
    // (enc_dec_process.c:1866). True on every allintra frame AND on a
    // video-mode key frame.
    is_islice: bool,
    // C `pcs->coeff_lvl` (`derive_inter_coeff_level`, md_config_process.c:650)
    // for the same gate — NORMAL/HIGH engage the clamp. Inert when
    // `is_islice`.
    coeff_lvl: crate::quant::CoeffLvl,
    // C `ctx->disallow_4x4` for THIS superblock — `pic_disallow_4x4` after
    // `set_depth_removal_level_controls` may have set it (enc_mode_config.c:
    // 3270-3278).
    disallow_4x4: bool,
    // C `ctx->disallow_8x8` (`get_disallow_8x8_{default,allintra}`) — false
    // on every reachable arm.
    disallow_8x8: bool,
    // C `ctx->depth_removal_ctrls` for THIS superblock — zeroed on a key
    // frame, which is also what every pre-existing test wants.
    depth_removal: crate::port_enc_mode_config::common::DepthRemovalCtrls,
) -> RefScan {
    let mut max_pd0 = 0usize;
    let mut min_pd0 = 255usize;
    if ctrls.limit_to_pd0 != 0 {
        match sb_max_min {
            Some((mx, mn)) => {
                max_pd0 = mx;
                min_pd0 = mn;
            }
            None => root.max_min_picked(&mut max_pd0, &mut min_pd0),
        }
    } else {
        max_pd0 = 1;
        min_pd0 = 1;
    }
    let env = RefineEnv {
        ctrls,
        disallow_4x4,
        disallow_8x8,
        depth_removal,
        lambda,
        tables,
        max_pd0,
        min_pd0,
        // Includes C's lossless geometry cap (enc_dec_process.c:1492)
        // as well as its max_tx_size cap (:1814).
        max_sq: usize::from(max_tx_size.min(64)),
        sb_sq,
        ref_min_max_sq,
        is_islice,
        coeff_lvl,
    };
    refine_depth(&env, root, None, sb_x, sb_y).0
}

// ---------------------------------------------------------------------------
// Partition rates at real contexts
// ---------------------------------------------------------------------------
