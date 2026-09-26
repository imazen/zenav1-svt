use super::*;

/// C `SearchSite` / `SearchSiteConfig` (av1me.h) + `svt_av1_init3smotion_
/// compensation` (av1me.c:159-184): the 3-step-search 8-direction-per-step
/// offset table, `MAX_FIRST_STEP` down to `1` by halving (11 steps x 8 +
/// 1 origin = 89 sites total for the default `MAX_FIRST_STEP = 1024`).
/// `offset` is unused by this port (addresses are always recomputed from
/// `(mv_x, mv_y)` via [`window`] rather than accumulated incrementally --
/// see this section's header note) but kept on the struct for a faithful
/// 1:1 field mirror of C's `SearchSite`.
#[derive(Debug, Clone, Copy)]
pub struct SearchSite {
    pub mv_x: i32,
    pub mv_y: i32,
    /// C `ss->offset` (`mv_y * stride + mv_x`) -- NOT used by this port's
    /// address computation (see struct doc); kept for field parity.
    pub offset: isize,
}

#[derive(Debug, Clone)]
pub struct SearchSiteConfig {
    pub sites: Vec<SearchSite>,
    pub searches_per_step: usize,
}

pub fn init_search_sites(stride: usize) -> SearchSiteConfig {
    let mut sites = Vec::with_capacity(89);
    sites.push(SearchSite {
        mv_x: 0,
        mv_y: 0,
        offset: 0,
    });
    let mut len = MAX_FIRST_STEP;
    while len > 0 {
        let ss_mvs: [(i32, i32); 8] = [
            (0, -len),
            (0, len),
            (-len, 0),
            (len, 0),
            (-len, -len),
            (len, -len),
            (-len, len),
            (len, len),
        ];
        for (mv_x, mv_y) in ss_mvs {
            sites.push(SearchSite {
                mv_x,
                mv_y,
                offset: mv_y as isize * stride as isize + mv_x as isize,
            });
        }
        len /= 2;
    }
    SearchSiteConfig {
        sites,
        searches_per_step: 8,
    }
}

/// C `svt_av1_diamond_search_sad_c` (av1me.c:291-420, EXPORTED symbol --
/// strongest evidence tier of this section).
///
/// C's signature takes TWO logically distinct MVs: `ref_mv` (mutated in
/// place by `clamp_mv` into the search's full-pel STARTING point, then
/// copied into `best_mv`) and `center_mv` (eighth-pel, used ONLY to derive
/// `fcenter_mv = {center_mv->x>>3, center_mv->y>>3}` for
/// [`mvsad_err_cost`]'s rate term -- never used as a search seed). At this
/// module's one call site (`full_pixel_diamond`, IBC only) both resolve to
/// the SAME underlying value: `ref_mv` is passed as `mvp_full = dv_ref >>
/// 3` and `center_mv` is passed as `dv_ref` itself, so `fcenter_mv ==
/// ref_mv` exactly. This port folds them into the single parameter
/// `center_eighth_pel` (= `dv_ref`) and derives BOTH the full-pel seed
/// (via `>>3` + clamp, mirroring `clamp_mv(ref_mv, ...)`) and the cost
/// reference (`fcenter_mv`) from it -- faithful to this vertical's actual
/// data flow, not a general-purpose `diamond_search_sad_c` binding (a
/// general port would need the two kept separate; document this if ever
/// reused outside IBC).
///
/// C accumulates `best_address` incrementally (`best_address +=
/// ss[best_site].offset`) to avoid recomputing a pointer from scratch each
/// improvement; this port recomputes the absolute address from `(best_x,
/// best_y)` via [`window`] on every pixel read instead -- provably
/// equivalent (both resolve to the SAME absolute position after the same
/// sequence of site moves) and simpler than threading a second piece of
/// mutable state that must stay in lock-step with `(best_x, best_y)`.
///
/// The `#if defined(NEW_DIAMOND_SEARCH)` refinement loop (av1me.c:391-408)
/// is DEAD CODE in the reference build (`NEW_DIAMOND_SEARCH` is never
/// `#define`d anywhere in `Source/`) and is NOT translated.
#[allow(clippy::too_many_arguments)]
pub fn diamond_search_sad(
    pic: &[u8],
    stride: usize,
    block_origin: (i32, i32),
    bw: usize,
    bh: usize,
    cfg: &SearchSiteConfig,
    center_eighth_pel: Mv,
    mv_limits: FullMvLimits,
    search_param: i32,
    sad_per_bit: i32,
    tables: &MvCostTables,
    approx_inter_rate: bool,
) -> (i32, i32, i32, i32) {
    // Returns (best_x, best_y, best_sad, num00).
    let fcenter_x = i32::from(center_eighth_pel.x) >> 3;
    let fcenter_y = i32::from(center_eighth_pel.y) >> 3;
    let mut best_x = fcenter_x.clamp(mv_limits.col_min, mv_limits.col_max);
    let mut best_y = fcenter_y.clamp(mv_limits.row_min, mv_limits.row_max);
    let (start_x, start_y) = (best_x, best_y);
    let mut num00 = 0i32;

    let ss = &cfg.sites[(search_param as usize) * cfg.searches_per_step..];
    let tot_steps = (cfg.sites.len() / cfg.searches_per_step) as i32 - search_param;

    let what = window(pic, stride, block_origin, 0, 0);
    let mut bestsad = svtav1_dsp::sad::sad(
        what,
        stride,
        window(pic, stride, block_origin, best_x, best_y),
        stride,
        bw,
        bh,
    ) as i32
        + mvsad_err_cost(
            best_x,
            best_y,
            fcenter_x,
            fcenter_y,
            sad_per_bit,
            approx_inter_rate,
            tables,
        );

    let mut i = 1usize;
    let mut best_site = 0usize;
    let mut last_site = 0usize;

    for _step in 0..tot_steps {
        let all_in = (best_y + ss[i].mv_y) > mv_limits.row_min
            && (best_y + ss[i + 1].mv_y) < mv_limits.row_max
            && (best_x + ss[i + 2].mv_x) > mv_limits.col_min
            && (best_x + ss[i + 3].mv_x) < mv_limits.col_max;

        if all_in {
            for _ in (0..cfg.searches_per_step).step_by(4) {
                for t in 0..4 {
                    let this_x = best_x + ss[i + t].mv_x;
                    let this_y = best_y + ss[i + t].mv_y;
                    let mut sad = svtav1_dsp::sad::sad(
                        what,
                        stride,
                        window(pic, stride, block_origin, this_x, this_y),
                        stride,
                        bw,
                        bh,
                    ) as i32;
                    if sad < bestsad {
                        sad += mvsad_err_cost(
                            this_x,
                            this_y,
                            fcenter_x,
                            fcenter_y,
                            sad_per_bit,
                            approx_inter_rate,
                            tables,
                        );
                        if sad < bestsad {
                            bestsad = sad;
                            best_site = i + t;
                        }
                    }
                }
                i += 4;
            }
        } else {
            for _ in 0..cfg.searches_per_step {
                let this_x = best_x + ss[i].mv_x;
                let this_y = best_y + ss[i].mv_y;
                if is_mv_in(mv_limits, this_x, this_y) {
                    let mut sad = svtav1_dsp::sad::sad(
                        what,
                        stride,
                        window(pic, stride, block_origin, this_x, this_y),
                        stride,
                        bw,
                        bh,
                    ) as i32;
                    if sad < bestsad {
                        sad += mvsad_err_cost(
                            this_x,
                            this_y,
                            fcenter_x,
                            fcenter_y,
                            sad_per_bit,
                            approx_inter_rate,
                            tables,
                        );
                        if sad < bestsad {
                            bestsad = sad;
                            best_site = i;
                        }
                    }
                }
                i += 1;
            }
        }
        if best_site != last_site {
            best_y += ss[best_site].mv_y;
            best_x += ss[best_site].mv_x;
            last_site = best_site;
        } else if (best_x, best_y) == (start_x, start_y) {
            num00 += 1;
        }
    }

    (best_x, best_y, bestsad, num00)
}

/// C `svt_av1_refining_search_sad` (av1me.c:420-460ish, `static`): 1-away
/// 4-neighbor diamond refinement, up to `search_range` iterations. C names
/// its own cost-scale parameter `error_per_bit`, but every call site in
/// this vertical (`full_pixel_diamond`) actually passes `sadpb` (the
/// SAD-domain per-bit value) into it, and the parameter flows straight
/// into [`mvsad_err_cost`]'s `sad_per_bit` argument -- so this port names
/// it `sad_per_bit` directly rather than carrying the C misnomer forward.
#[allow(clippy::too_many_arguments)]
pub fn refining_search_sad(
    pic: &[u8],
    stride: usize,
    block_origin: (i32, i32),
    bw: usize,
    bh: usize,
    start_x: i32,
    start_y: i32,
    center_eighth_pel: Mv,
    mv_limits: FullMvLimits,
    search_range: i32,
    sad_per_bit: i32,
    tables: &MvCostTables,
    approx_inter_rate: bool,
) -> (i32, i32, i32) {
    const NEIGHBORS: [(i32, i32); 4] = [(0, -1), (-1, 0), (1, 0), (0, 1)];
    let fcenter_x = i32::from(center_eighth_pel.x) >> 3;
    let fcenter_y = i32::from(center_eighth_pel.y) >> 3;
    let mut x = start_x;
    let mut y = start_y;
    let what = window(pic, stride, block_origin, 0, 0);
    let mut best_sad = svtav1_dsp::sad::sad(
        what,
        stride,
        window(pic, stride, block_origin, x, y),
        stride,
        bw,
        bh,
    ) as i32
        + mvsad_err_cost(
            x,
            y,
            fcenter_x,
            fcenter_y,
            sad_per_bit,
            approx_inter_rate,
            tables,
        );

    for _ in 0..search_range {
        let mut best_site: Option<usize> = None;
        for (j, (dx, dy)) in NEIGHBORS.iter().enumerate() {
            let nx = x + dx;
            let ny = y + dy;
            if is_mv_in(mv_limits, nx, ny) {
                let mut sad = svtav1_dsp::sad::sad(
                    what,
                    stride,
                    window(pic, stride, block_origin, nx, ny),
                    stride,
                    bw,
                    bh,
                ) as i32;
                if sad < best_sad {
                    sad += mvsad_err_cost(
                        nx,
                        ny,
                        fcenter_x,
                        fcenter_y,
                        sad_per_bit,
                        approx_inter_rate,
                        tables,
                    );
                    if sad < best_sad {
                        best_sad = sad;
                        best_site = Some(j);
                    }
                }
            }
        }
        match best_site {
            None => break,
            Some(j) => {
                let (dx, dy) = NEIGHBORS[j];
                x += dx;
                y += dy;
            }
        }
    }
    (x, y, best_sad)
}

/// C `full_pixel_diamond` (av1me.c:489-556, `static`): the diamond search
/// entry. Runs [`diamond_search_sad`] at `step_param`, then re-runs it at
/// deepening `step_param + n` levels (each restarting from the SAME
/// `center_eighth_pel`-derived seed -- C reuses the same `mvp_full`
/// pointer across every `svt_av1_diamond_search_sad_c` call in this
/// function, and `clamp_mv` is idempotent after its first application, so
/// every level searches outward from one fixed origin, keeping whichever
/// level's result scores best) while `num00` (consecutive "didn't move"
/// steps) permits skipping ahead, then -- unless a shallow level already
/// exhausted `further_steps` or a deep level's own `num00` used up the
/// remaining budget -- a final 1-away [`refining_search_sad`] pass seeded
/// from the best point found so far. Returns `(best_x, best_y, best_cost)`
/// full-pel, where `best_cost` is [`get_mvpred_var`]'s precise
/// (`use_mvcost=true`) value at the winning point (C recomputes this via
/// `svt_av1_get_mvpred_var` after EVERY `diamond_search_sad_c`/
/// `refining_search_sad` call, never trusting the raw SAD return directly
/// for the `bestsme` comparison -- mirrored exactly below).
#[allow(clippy::too_many_arguments)]
pub fn full_pixel_diamond(
    pic: &[u8],
    stride: usize,
    block_origin: (i32, i32),
    bw: usize,
    bh: usize,
    cfg: &SearchSiteConfig,
    mv_limits: FullMvLimits,
    step_param: i32,
    sadpb: i32,
    further_steps: i32,
    do_refine_in: bool,
    center_eighth_pel: Mv,
    tables: &MvCostTables,
    error_per_bit: i32,
    approx_inter_rate: bool,
) -> (i32, i32, i32) {
    let (mut best_x, mut best_y, sad0, mut n) = diamond_search_sad(
        pic,
        stride,
        block_origin,
        bw,
        bh,
        cfg,
        center_eighth_pel,
        mv_limits,
        step_param,
        sadpb,
        tables,
        approx_inter_rate,
    );
    // C: `if (bestsme < INT_MAX) bestsme = svt_av1_get_mvpred_var(...)` —
    // the diamond's raw SAD return is never trusted for the cross-stage
    // compare, but the INT_MAX "not found" sentinel is passed through.
    let mut bestsme = if sad0 < i32::MAX {
        get_mvpred_var(
            pic,
            stride,
            block_origin,
            bw,
            bh,
            best_x,
            best_y,
            center_eighth_pel,
            tables,
            error_per_bit,
            approx_inter_rate,
            true,
        )
    } else {
        sad0
    };

    let mut do_refine = do_refine_in;
    if n > further_steps {
        do_refine = false;
    }

    // C's num00 skip (av1me.c:511-537): a diamond call reporting `num00`
    // no-move coarse steps causes the NEXT `num00` step-param levels to be
    // SKIPPED ENTIRELY (no diamond call, no candidate compare) — they
    // would re-search the same neighborhood. FIXED (chunk 5): the original
    // bulk-port translation ran every level unconditionally, evaluating
    // (and potentially adopting) candidates C never visits.
    let mut num00 = 0i32;
    while n < further_steps {
        n += 1;
        if num00 > 0 {
            num00 -= 1;
        } else {
            let (cand_x, cand_y, cand_sad, this_num00) = diamond_search_sad(
                pic,
                stride,
                block_origin,
                bw,
                bh,
                cfg,
                center_eighth_pel,
                mv_limits,
                step_param + n,
                sadpb,
                tables,
                approx_inter_rate,
            );
            num00 = this_num00;
            let thissme = if cand_sad < i32::MAX {
                get_mvpred_var(
                    pic,
                    stride,
                    block_origin,
                    bw,
                    bh,
                    cand_x,
                    cand_y,
                    center_eighth_pel,
                    tables,
                    error_per_bit,
                    approx_inter_rate,
                    true,
                )
            } else {
                cand_sad
            };
            if num00 > further_steps - n {
                do_refine = false;
            }
            if thissme < bestsme {
                bestsme = thissme;
                best_x = cand_x;
                best_y = cand_y;
            }
        }
    }

    if do_refine {
        const SEARCH_RANGE: i32 = 8;
        let (rx, ry, rsad) = refining_search_sad(
            pic,
            stride,
            block_origin,
            bw,
            bh,
            best_x,
            best_y,
            center_eighth_pel,
            mv_limits,
            SEARCH_RANGE,
            sadpb,
            tables,
            approx_inter_rate,
        );
        let thissme = if rsad < i32::MAX {
            get_mvpred_var(
                pic,
                stride,
                block_origin,
                bw,
                bh,
                rx,
                ry,
                center_eighth_pel,
                tables,
                error_per_bit,
                approx_inter_rate,
                true,
            )
        } else {
            rsad
        };
        if thissme < bestsme {
            bestsme = thissme;
            best_x = rx;
            best_y = ry;
        }
    }

    (best_x, best_y, bestsme)
}

/// C `exhaustive_mesh_search` (av1me.c:212-290, `static`): a full raster
/// scan of the `[-range, range]` window around `center_full_pel` (clamped
/// into `mv_limits`), stepping by `step` rows and (`step>1 ? step : 4`)
/// columns. `ref_mv_full` is the full-pel cost reference (matches C's
/// `ref_mv` param -- always the caller's full-pel `ref_mv_fp` in this
/// vertical, itself `ref_mv_eighth_pel >> 3`, see
/// [`intrabc_full_pixel_exhaustive`]).
///
/// C's `x->second_best_mv` write in this function is never READ anywhere
/// in the IBC call chain (`full_pixel_search` -> `intrabc_full_pixel_
/// exhaustive` -> here); it exists for OTHER callers of this shared ME
/// primitive (regular inter ME). Not carried by this port.
///
/// PORT-NOTE(unverified): when `step == 1` and the tail of a row has fewer
/// than 4 remaining columns (`c + 3 > end_col`), C's own scalar fallback
/// loop is `for (i = 0; i < end_col - c; ++i)` -- note the STRICT `<`
/// against `end_col - c`, NOT `end_col - c + 1`. This means column
/// `end_col` itself is skipped by the tail branch whenever it is reached
/// via that branch (e.g. exactly 1 column remaining: `end_col - c == 0`,
/// zero iterations, `end_col` never visited). Reproduced bug-for-bug
/// below (`n = (end_col - c).max(0)`, not `+1`) rather than "fixed" --
/// see `CLAUDE.md`'s "translate exactly" mandate. Only affects the last
/// 0-3 columns of a mesh-search row when the window width isn't a
/// multiple of 4; `range`/`interval` come from [`IbcCtrls::mesh_patterns`]
/// (always multiples of 8 or the terminal 1-wide refinement pass, so this
/// mistranscription-shaped gap is rarely if ever exercised, but is kept
/// faithful regardless).
#[allow(clippy::too_many_arguments)]
pub fn exhaustive_mesh_search(
    pic: &[u8],
    stride: usize,
    block_origin: (i32, i32),
    bw: usize,
    bh: usize,
    ref_mv_full: (i32, i32),
    range: i32,
    step: i32,
    sad_per_bit: i32,
    center_full_pel: (i32, i32),
    mv_limits: FullMvLimits,
    tables: &MvCostTables,
    approx_inter_rate: bool,
) -> ((i32, i32), i32) {
    debug_assert!(step >= 1);
    let col_step = if step > 1 { step } else { 4 };
    let what = window(pic, stride, block_origin, 0, 0);

    let fcx = center_full_pel
        .0
        .clamp(mv_limits.col_min, mv_limits.col_max);
    let fcy = center_full_pel
        .1
        .clamp(mv_limits.row_min, mv_limits.row_max);
    let mut best_mv = (fcx, fcy);
    let mut best_sad = svtav1_dsp::sad::sad(
        what,
        stride,
        window(pic, stride, block_origin, fcx, fcy),
        stride,
        bw,
        bh,
    ) as i32
        + mvsad_err_cost(
            fcx,
            fcy,
            ref_mv_full.0,
            ref_mv_full.1,
            sad_per_bit,
            approx_inter_rate,
            tables,
        );

    let start_row = (-range).max(mv_limits.row_min - fcy);
    let start_col = (-range).max(mv_limits.col_min - fcx);
    let end_row = range.min(mv_limits.row_max - fcy);
    let end_col = range.min(mv_limits.col_max - fcx);

    let mut r = start_row;
    while r <= end_row {
        let mut c = start_col;
        while c <= end_col {
            let n = if step > 1 {
                1
            } else if c + 3 <= end_col {
                4
            } else {
                (end_col - c).max(0)
            };
            // C uses sdx4df only for a complete four-position group. Keep
            // its scalar remainder and sequential strict-< winner updates.
            let batch = if n == 4 {
                Some(svtav1_dsp::me_sad::block_sad_x4(
                    what,
                    stride,
                    core::array::from_fn(|i| {
                        window(pic, stride, block_origin, fcx + c + i as i32, fcy + r)
                    }),
                    stride,
                    bw,
                    bh,
                ))
            } else {
                None
            };
            for i in 0..n {
                let mx = fcx + c + i;
                let my = fcy + r;
                let sad = batch.map_or_else(
                    || {
                        svtav1_dsp::sad::sad(
                            what,
                            stride,
                            window(pic, stride, block_origin, mx, my),
                            stride,
                            bw,
                            bh,
                        )
                    },
                    |values| values[i as usize],
                ) as i32;
                if sad < best_sad {
                    let sad2 = sad
                        + mvsad_err_cost(
                            mx,
                            my,
                            ref_mv_full.0,
                            ref_mv_full.1,
                            sad_per_bit,
                            approx_inter_rate,
                            tables,
                        );
                    if sad2 < best_sad {
                        best_sad = sad2;
                        best_mv = (mx, my);
                    }
                }
            }
            c += col_step;
        }
        r += step;
    }
    (best_mv, best_sad)
}

/// C `MIN_RANGE` / `MAX_RANGE` / `MIN_INTERVAL` (av1me.c:558-560).
pub(super) const MIN_RANGE: i32 = 7;
pub(super) const MAX_RANGE: i32 = 256;
pub(super) const MIN_INTERVAL: i32 = 1;

/// C `intrabc_full_pixel_exhaustive` (av1me.c:566-625, `static`): runs
/// [`exhaustive_mesh_search`] over `ctrls.mesh_patterns`, adapting the
/// first ring's range/interval to the center MV's magnitude, then
/// progressively refining through the remaining configured rings (a
/// `range == 0` entry -- the zero-default tail of [`IbcCtrls::mesh_
/// patterns`] at levels 6/7, or any level's unused trailing slots --
/// terminates the refinement early; an `interval == 1` ring is always the
/// last one run). Returns `None` when the validated first ring is
/// malformed (`range`/`interval` outside `[MIN_RANGE,MAX_RANGE]`/
/// `[MIN_INTERVAL,range]`) -- C's `INT_MAX` "not found" sentinel, e.g.
/// self-consistently a no-op whenever `mesh_patterns[0]` is left
/// zero-defaulted (levels 6/7, see [`IbcCtrls::for_level`]'s PORT-NOTE).
/// `center_full_pel` is `x->best_mv` at the call site (i.e. the diamond
/// search's winner, full-pel).
#[allow(clippy::too_many_arguments)]
pub fn intrabc_full_pixel_exhaustive(
    pic: &[u8],
    stride: usize,
    block_origin: (i32, i32),
    bw: usize,
    bh: usize,
    ctrls: &IbcCtrls,
    center_full_pel: (i32, i32),
    sad_per_bit: i32,
    ref_mv_eighth_pel: Mv,
    mv_limits: FullMvLimits,
    tables: &MvCostTables,
    error_per_bit: i32,
    approx_inter_rate: bool,
) -> Option<((i32, i32), i32)> {
    let ref_mv_full = (
        i32::from(ref_mv_eighth_pel.x) >> 3,
        i32::from(ref_mv_eighth_pel.y) >> 3,
    );

    let mut range = ctrls.mesh_patterns[0].range;
    let interval0 = ctrls.mesh_patterns[0].interval;
    if !(MIN_RANGE..=MAX_RANGE).contains(&range) || !(MIN_INTERVAL..=range).contains(&interval0) {
        return None;
    }
    let base_interval_div = range / interval0;

    let mv_mag = center_full_pel.0.abs().max(center_full_pel.1.abs());
    range = range.max((5 * mv_mag) / 4).min(MAX_RANGE);
    let interval = interval0.max(range / base_interval_div);

    let (mut search_mv, mut best_cost) = exhaustive_mesh_search(
        pic,
        stride,
        block_origin,
        bw,
        bh,
        ref_mv_full,
        range,
        interval,
        sad_per_bit,
        center_full_pel,
        mv_limits,
        tables,
        approx_inter_rate,
    );

    // C's own `interval` local is never re-read after this gate (each
    // refinement ring below uses `pattern.interval` directly, and the
    // gate itself is checked ONCE, not per-iteration) -- matched exactly,
    // no reassignment inside the loop.
    if interval > MIN_INTERVAL && range > MIN_RANGE {
        for pattern in &ctrls.mesh_patterns[1..MAX_MESH_STEP] {
            if pattern.range == 0 {
                break;
            }
            let (mv2, cost2) = exhaustive_mesh_search(
                pic,
                stride,
                block_origin,
                bw,
                bh,
                ref_mv_full,
                pattern.range,
                pattern.interval,
                sad_per_bit,
                search_mv,
                mv_limits,
                tables,
                approx_inter_rate,
            );
            search_mv = mv2;
            best_cost = cost2;
            if pattern.interval == 1 {
                break;
            }
        }
    }

    if best_cost < i32::MAX {
        best_cost = get_mvpred_var(
            pic,
            stride,
            block_origin,
            bw,
            bh,
            search_mv.0,
            search_mv.1,
            ref_mv_eighth_pel,
            tables,
            error_per_bit,
            approx_inter_rate,
            true,
        );
    }
    Some((search_mv, best_cost))
}

/// C `svt_av1_full_pixel_search` (av1me.c:1115-1155, EXPORTED symbol): the
/// diamond-then-optional-mesh entry point. `mvp_full` (the diamond seed)
/// is `ref_mv_eighth_pel >> 3` in every call site of this vertical --
/// see [`diamond_search_sad`]'s doc for why this port folds "seed" and
/// "cost center" into the one `ref_mv_eighth_pel` parameter.
///
/// The `(uint64_t)~0` "always mesh" encoding ([`IbcCtrls::for_level`]
/// levels 6/7): C narrows `intrabc_ctrls->exhaustive_mesh_thresh` (u64)
/// to a plain `int` via `(int)pcs->ppcs->intrabc_ctrls.exhaustive_mesh_
/// thresh` BEFORE the bsize-scaled right-shift -- `(int)(uint64_t)~0`
/// truncates to the low 32 bits, reinterpreted as `-1` (two's complement,
/// implementation-defined by the C standard but universal in practice on
/// every mainstream target, matching Rust's `as i32` narrowing cast
/// exactly). `-1 >> n == -1` for any `n` (arithmetic shift, sign-extends),
/// so `var > -1` is true for every realistic non-negative SAD/variance
/// `var`, i.e. mesh search always fires -- exactly the comment's intent
/// ("set to INF to always allow mesh search"), achieved through this
/// specific integer-truncation trick rather than a dedicated bool flag.
#[allow(clippy::too_many_arguments)]
pub fn full_pixel_search(
    pic: &[u8],
    stride: usize,
    block_origin: (i32, i32),
    bw: usize,
    bh: usize,
    cfg: &SearchSiteConfig,
    mv_limits: FullMvLimits,
    ref_mv_eighth_pel: Mv,
    sad_per_bit: i32,
    error_per_bit: i32,
    ctrls: &IbcCtrls,
    mi_size_wide_log2: u32,
    mi_size_high_log2: u32,
    tables: &MvCostTables,
    approx_inter_rate: bool,
) -> (i32, i32, i32) {
    const STEP_PARAM: i32 = 0;
    let (mut best_x, mut best_y, mut var) = full_pixel_diamond(
        pic,
        stride,
        block_origin,
        bw,
        bh,
        cfg,
        mv_limits,
        STEP_PARAM,
        sad_per_bit,
        MAX_MVSEARCH_STEPS - 1 - STEP_PARAM,
        true,
        ref_mv_eighth_pel,
        tables,
        error_per_bit,
        approx_inter_rate,
    );

    // `10 - (mi_size_wide_log2 + mi_size_high_log2)` is C `int` arithmetic
    // (uint8_t operands promote to signed `int`) and could in principle go
    // negative for a hypothetical bsize with mi-log2 sum > 10 -- shifting
    // by a negative amount would be C UB. Every real BLOCK_SIZES_ALL entry
    // has `mi_size_wide_log2 + mi_size_high_log2 <= 10` (the max, 5+5, is
    // BLOCK_128X128 itself), so the shift amount is always in `0..=10` in
    // practice; assert that invariant rather than silently wrapping a u32
    // subtraction.
    debug_assert!(mi_size_wide_log2 + mi_size_high_log2 <= 10);
    let mut exhaustive_mesh_thresh = ctrls.exhaustive_mesh_thresh as i32; // see fn doc
    exhaustive_mesh_thresh >>= 10 - (mi_size_wide_log2 + mi_size_high_log2);

    let mut run_mesh_search = var > exhaustive_mesh_thresh;
    // C's `mvp_full` was CLAMPED IN PLACE into `x->mv_limits` by
    // `clamp_mv(ref_mv, ...)` inside the first `svt_av1_diamond_search_
    // sad_c` call (av1me.c:318 — same pointer reused across every level),
    // so the :1142 `full_pel_mv_diff` reads the CLAMPED seed, not the raw
    // `dv_ref >> 3`. FIXED (chunk 5): the original translation compared
    // against the unclamped seed — divergent whenever the direction box
    // excludes the dv_ref (e.g. LEFT direction at an SB's first column,
    // where col_max = -bw < 0 = dv_ref.x >> 3).
    let mvp_full_x =
        (i32::from(ref_mv_eighth_pel.x) >> 3).clamp(mv_limits.col_min, mv_limits.col_max);
    let mvp_full_y =
        (i32::from(ref_mv_eighth_pel.y) >> 3).clamp(mv_limits.row_min, mv_limits.row_max);
    let full_pel_mv_diff = (mvp_full_x - best_x).abs().max((mvp_full_y - best_y).abs());
    if full_pel_mv_diff <= ctrls.mesh_search_mv_diff_threshold {
        run_mesh_search = false;
    }

    if run_mesh_search
        && let Some(((ex_x, ex_y), var_ex)) = intrabc_full_pixel_exhaustive(
            pic,
            stride,
            block_origin,
            bw,
            bh,
            ctrls,
            (best_x, best_y),
            sad_per_bit,
            ref_mv_eighth_pel,
            mv_limits,
            tables,
            error_per_bit,
            approx_inter_rate,
        )
        && var_ex < var
    {
        best_x = ex_x;
        best_y = ex_y;
        var = var_ex;
    }

    (best_x, best_y, var)
}

// =============================================================================
// §4b. Hash search -- the SELECTION algorithm only. The hash TABLE itself
// (CRC-based block hashing + bucket storage) is NOT translated -- see the
// module doc's "documented only" list. `hash_search_eligible` +
// `hash_search_best_in_bucket` translate the reachable, pure parts of
// `svt_av1_intrabc_hash_search` (av1me.c:1056-1114); the caller is
// responsible for producing the bucket (via an unported `HashTable`
// equivalent) and the block's own `(hash_value1, hash_value2)` (via an
// unported `svt_av1_get_block_hash_value`, hash_motion.c:309+). Passing an
// empty/`None` bucket is always LEGAL and just means every DV this port
// finds comes from [`full_pixel_search`] instead -- correct, only slower.
// =============================================================================
