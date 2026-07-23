//! Differential parity: the IntraBC DV pixel search (`svtav1-encoder/src/
//! intrabc.rs` §4) vs the REAL exported C functions (IBC chunk 5,
//! docs/ibc-port-map.md §D).
//!
//! C oracles:
//!   - `svt_aom_mefn_ptr[bsize].sdf/vf`       (av1me.c:24-62, the RTCD-
//!     resolved kernels the search binds — locks the port's SAD +
//!     VARIANCE against the exact fns C dispatches)
//!   - `svt_av1_diamond_search_sad_c`         (av1me.c:291, EXPORTED)
//!   - `svt_av1_full_pixel_search`            (av1me.c:1115, EXPORTED —
//!     diamond + num00 skip + refine + the either/or mesh)
//!   - `svt_av1_intrabc_hash_search`          (av1me.c:1056, EXPORTED —
//!     end-to-end vs the chunk-4 C hash table)
//!   - the `intra_bc_search` driver (mode_decision.c:2976, static) via
//!     the ref_shims.c verbatim transcription over the real exported fns
//!
//! Cost tables come from C's own build chain (`svt_aom_estimate_mv_rate`)
//! on the C side and the chunk-2-locked `build_nmv_cost_table` on the
//! port side — proven equal in c_parity_intrabc.rs, so any divergence
//! here is in the search itself.

use svtav1_cref as cref;
use svtav1_encoder::intrabc;
use svtav1_encoder::intrabc_hash;
use svtav1_entropy::mv_coding::{
    CLASS0_SIZE, MV_CLASSES, MV_FP_SIZE, MV_OFFSET_BITS, MvSubpelPrecision, NmvComponent,
    NmvContext,
};
use svtav1_types::motion::Mv;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn random_cdf(rng: &mut Rng, out: &mut [u16]) {
    let nsymbs = out.len() - 1;
    loop {
        let mut cuts: Vec<u16> = (0..nsymbs - 1).map(|_| 1 + rng.below(32766) as u16).collect();
        cuts.sort_unstable_by(|a, b| b.cmp(a));
        cuts.dedup();
        if cuts.len() == nsymbs - 1 {
            out[..nsymbs - 1].copy_from_slice(&cuts);
            break;
        }
    }
    out[nsymbs - 1] = 0;
    out[nsymbs] = rng.below(33) as u16;
}

fn random_nmv_context(rng: &mut Rng) -> NmvContext {
    let mut c = NmvComponent {
        classes_cdf: [0; MV_CLASSES + 1],
        class0_fp_cdf: [[0; MV_FP_SIZE + 1]; CLASS0_SIZE],
        fp_cdf: [0; MV_FP_SIZE + 1],
        sign_cdf: [0; 3],
        class0_hp_cdf: [0; 3],
        hp_cdf: [0; 3],
        class0_cdf: [0; CLASS0_SIZE + 1],
        bits_cdf: [[0; 3]; MV_OFFSET_BITS],
    };
    random_cdf(rng, &mut c.classes_cdf);
    for row in &mut c.class0_fp_cdf {
        random_cdf(rng, row);
    }
    random_cdf(rng, &mut c.fp_cdf);
    random_cdf(rng, &mut c.sign_cdf);
    random_cdf(rng, &mut c.class0_hp_cdf);
    random_cdf(rng, &mut c.hp_cdf);
    random_cdf(rng, &mut c.class0_cdf);
    for row in &mut c.bits_cdf {
        random_cdf(rng, row);
    }
    let mut ctx = NmvContext::default();
    random_cdf(rng, &mut ctx.joints_cdf);
    ctx.comps = [c.clone(), c];
    ctx
}

fn flatten_nmv(ctx: &NmvContext) -> Vec<u16> {
    let mut flat: Vec<u16> = Vec::with_capacity(cref::NMV_FLAT_LEN);
    flat.extend_from_slice(&ctx.joints_cdf);
    for comp in &ctx.comps {
        flat.extend_from_slice(&comp.classes_cdf);
        for fp in &comp.class0_fp_cdf {
            flat.extend_from_slice(fp);
        }
        flat.extend_from_slice(&comp.fp_cdf);
        flat.extend_from_slice(&comp.sign_cdf);
        flat.extend_from_slice(&comp.class0_hp_cdf);
        flat.extend_from_slice(&comp.hp_cdf);
        flat.extend_from_slice(&comp.class0_cdf);
        for b in &comp.bits_cdf {
            flat.extend_from_slice(b);
        }
    }
    flat
}

/// Both sides' cost tables from ONE random ndvc: C's via the real build
/// chain, the port's via the chunk-2-locked builder.
struct Costs {
    c_dv_joint: Vec<i32>,
    c_dv0: Vec<i32>,
    c_dv1: Vec<i32>,
    port: intrabc::MvCostTables,
    errorperbit: i32,
}

fn make_costs(rng: &mut Rng, errorperbit: i32) -> Costs {
    let ndvc = if rng.below(2) == 0 { NmvContext::default() } else { random_nmv_context(rng) };
    let flat = flatten_nmv(&ndvc);
    let c = cref::estimate_mv_rate(false, true, false, None, Some(&flat), -1);
    let (c0, c1) = c.dv_costs.split_at(cref::MV_VALS);
    Costs {
        c_dv_joint: c.dv_joint.to_vec(),
        c_dv0: c0.to_vec(),
        c_dv1: c1.to_vec(),
        port: intrabc::build_nmv_cost_table(&ndvc, MvSubpelPrecision::None),
        errorperbit,
    }
}

impl Costs {
    fn as_search_costs(&self, approx: bool) -> cref::SearchCosts<'_> {
        cref::SearchCosts {
            dv_joint: &self.c_dv_joint,
            dv_cost0: &self.c_dv0,
            dv_cost1: &self.c_dv1,
            errorperbit: self.errorperbit,
            approx_inter_rate: approx,
        }
    }
}

/// SVT BlockSize enum values for the square + rect sizes used here.
fn bsize_of(bw: i32, bh: i32) -> usize {
    match (bw, bh) {
        (4, 4) => 0,
        (8, 8) => 3,
        (8, 16) => 4,
        (16, 8) => 5,
        (16, 16) => 6,
        (16, 32) => 7,
        (32, 16) => 8,
        (32, 32) => 9,
        (64, 64) => 12,
        (8, 32) => 18,
        (32, 8) => 19,
        _ => panic!("unmapped bsize {bw}x{bh}"),
    }
}

/// Screen-content-ish frame with strong exact repeats (hash hits + good
/// DV targets), bars, and a noise band (pixel-search exercise).
///
/// NOTE the frame must be WIDE in SB64s: `is_dv_valid`'s already-coded-SB
/// delay (`INTRABC_DELAY_SB64 = 4`, adaptive_mv_pred.c:1908+) rejects any
/// DV whose source is fewer than 4 SB64s behind the active one in raster
/// order — on a frame only 3 SB64s wide NO DV is ever valid (measured:
/// both sides agreed on zero candidates everywhere). Real screen content
/// is >= 10 SB64s wide; the driver/hash fixtures below use 640px.
fn screen_frame(rng: &mut Rng, w: usize, h: usize, stride: usize) -> Vec<u8> {
    let mut pic = vec![128u8; stride * h];
    // Repeating 16x16 tile pattern over most of the frame (strong exact
    // repeats at every 16-multiple offset -> dense hash hits + far-away
    // identical blocks that satisfy the SB64 delay).
    let mut tile = [[0u8; 16]; 16];
    for (r, row) in tile.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            *v = ((r * 13 + c * 7) % 200 + 20) as u8;
        }
    }
    for y in 0..h {
        for x in 0..w {
            pic[y * stride + x] = tile[y % 16][x % 16];
        }
    }
    // A vertical-bars band (structured non-repeating-in-y content).
    for y in h / 2..h / 2 + h / 6 {
        for x in 0..w {
            pic[y * stride + x] = if (x / 4) % 2 == 0 { 40 } else { 210 };
        }
    }
    // Bottom sixth: noise (unique content, hash misses, diamond work).
    for y in h - h / 6..h {
        for x in 0..w {
            pic[y * stride + x] = rng.below(256) as u8;
        }
    }
    pic
}

const MI: i32 = 4;

/// The direction limits + set_mv_search_range narrowing, shared verbatim
/// with the port (the C driver's :3046-3080). Returns None for an empty box.
fn narrowed_limits(
    dir: intrabc::IntrabcMotionDirection,
    tile: intrabc::TileMiBounds,
    mi_row: i32,
    mi_col: i32,
    bw: i32,
    bh: i32,
    sb_mi_size: i32,
    sb_log2: u32,
    dv_ref: Mv,
) -> Option<svtav1_types::motion::FullMvLimits> {
    let mut l = intrabc::direction_mv_limits(dir, tile, mi_row, mi_col, bw, bh, sb_mi_size, sb_log2);
    intrabc::set_mv_search_range(&mut l, dv_ref);
    if l.col_max < l.col_min || l.row_max < l.row_min {
        None
    } else {
        Some(l)
    }
}

// ---------------------------------------------------------------------------
// Kernel level: the exact sdf / vf the C search binds
// ---------------------------------------------------------------------------

#[test]
fn c_parity_search_kernels() {
    let mut rng = Rng(0x1BC5_EA2C_0001);
    let (w, h, stride) = (160usize, 128usize, 168usize);
    let pic = screen_frame(&mut rng, w, h, stride);
    let mut checked = 0u64;
    for &(bw, bh) in &[(4i32, 4i32), (8, 8), (16, 8), (8, 16), (16, 16), (32, 32), (64, 64), (32, 8), (8, 32)] {
        let bsize = bsize_of(bw, bh);
        for _ in 0..50 {
            let sx = rng.below((w - bw as usize) as u64) as usize;
            let sy = rng.below((h - bh as usize) as u64) as usize;
            let rx = rng.below((w - bw as usize) as u64) as usize;
            let ry = rng.below((h - bh as usize) as u64) as usize;
            let s = &pic[sy * stride + sx..];
            let r = &pic[ry * stride + rx..];
            let c_sad = cref::mefn_sdf(bsize, s, stride, r, stride);
            let rs_sad = svtav1_dsp::sad::sad(s, stride, r, stride, bw as usize, bh as usize);
            assert_eq!(rs_sad, c_sad, "sdf diverges {bw}x{bh} at ({sx},{sy})->({rx},{ry})");
            let c_var = cref::mefn_vf(bsize, s, stride, r, stride);
            let rs_var = intrabc::variance_of_diff(s, stride, r, stride, bw as usize, bh as usize);
            assert_eq!(rs_var, c_var, "vf diverges {bw}x{bh} at ({sx},{sy})->({rx},{ry})");
            checked += 1;
        }
    }
    assert_eq!(checked, 9 * 50);
}

// ---------------------------------------------------------------------------
// Diamond core (exported svt_av1_diamond_search_sad_c)
// ---------------------------------------------------------------------------

#[test]
fn c_parity_diamond_search() {
    let mut rng = Rng(0x1BC5_EA2C_0002);
    let (w, h, stride) = (448usize, 128usize, 456usize);
    let pic = screen_frame(&mut rng, w, h, stride);
    let (mi_rows, mi_cols) = ((h as i32) / MI, (w as i32) / MI);
    let tile = intrabc::TileMiBounds { mi_col_start: 0, mi_col_end: mi_cols, mi_row_start: 0, mi_row_end: mi_rows };
    let costs = make_costs(&mut rng, 800 >> 6);
    let sc = costs.as_search_costs(false);
    let cfg = intrabc::init_search_sites(stride);

    let mut nontrivial = 0u64;
    let mut num00_seen = 0u64;
    let mut checked = 0u64;
    for &(bw, bh) in &[(8i32, 8i32), (16, 16), (16, 8), (32, 32)] {
        let bsize = bsize_of(bw, bh);
        for iter in 0..60 {
            // Block position: anywhere with room above/left (row >= 2 SBs
            // occasionally to give the ABOVE box height).
            let bh_mi = bh / MI;
            let bw_mi = bw / MI;
            let mi_row = (bh_mi + rng.below((mi_rows - 2 * bh_mi).max(1) as u64) as i32) / bh_mi * bh_mi;
            let mi_col = (bw_mi + rng.below((mi_cols - 2 * bw_mi).max(1) as u64) as i32) / bw_mi * bw_mi;
            let dir = if rng.below(2) == 0 {
                intrabc::IntrabcMotionDirection::Above
            } else {
                intrabc::IntrabcMotionDirection::Left
            };
            let dv_ref = intrabc::find_ref_dv(tile, 16, mi_row);
            let Some(limits) = narrowed_limits(dir, tile, mi_row, mi_col, bw, bh, 16, 4, dv_ref) else {
                continue;
            };
            for search_param in [0i32, 1, 3, 5] {
                let block = (mi_col * MI, mi_row * MI);
                let (rs_x, rs_y, rs_sad, rs_n00) = intrabc::diamond_search_sad(
                    &pic,
                    stride,
                    block,
                    bw as usize,
                    bh as usize,
                    &cfg,
                    dv_ref,
                    limits,
                    search_param,
                    30,
                    &costs.port,
                    false,
                );
                let (c_x, c_y, c_sad, c_n00) = cref::diamond_search(
                    &pic,
                    stride,
                    block,
                    bsize,
                    (i32::from(dv_ref.x), i32::from(dv_ref.y)),
                    search_param,
                    30,
                    (limits.col_min, limits.col_max, limits.row_min, limits.row_max),
                    &sc,
                );
                assert_eq!(
                    (rs_x, rs_y, rs_sad, rs_n00),
                    (c_x, c_y, c_sad, c_n00),
                    "diamond diverges {bw}x{bh} mi=({mi_row},{mi_col}) dir={dir:?} sp={search_param} iter={iter}"
                );
                let seed_x = i32::from(dv_ref.x) >> 3;
                let seed_y = i32::from(dv_ref.y) >> 3;
                if (c_x, c_y) != (seed_x.clamp(limits.col_min, limits.col_max), seed_y.clamp(limits.row_min, limits.row_max)) {
                    nontrivial += 1;
                }
                num00_seen += c_n00.max(0) as u64;
                checked += 1;
            }
        }
    }
    assert!(checked > 250, "too few diamond cases ran: {checked}");
    assert!(nontrivial > 30, "diamond never moved off the seed: vacuous fixture");
    assert!(num00_seen > 0, "num00 never nonzero: skip path untested");
}

// ---------------------------------------------------------------------------
// full_pixel_search (diamond + num00 skip + refine + either/or mesh)
// ---------------------------------------------------------------------------

#[test]
fn c_parity_full_pixel_search() {
    let mut rng = Rng(0x1BC5_EA2C_0003);
    let (w, h, stride) = (448usize, 128usize, 448usize);
    let pic = screen_frame(&mut rng, w, h, stride);
    let (mi_rows, mi_cols) = ((h as i32) / MI, (w as i32) / MI);
    let tile = intrabc::TileMiBounds { mi_col_start: 0, mi_col_end: mi_cols, mi_row_start: 0, mi_row_end: mi_rows };
    let cfg = intrabc::init_search_sites(stride);

    // Mesh configurations: level-3 real ctrls (thresh 1<<20, patterns
    // (256,8)/(64,1)), forced-always (thresh 0, mv_diff -1), and
    // forced-never (huge mv_diff threshold kills it).
    let mesh_cfgs: [(u64, i32, [(i32, i32); 4]); 3] = [
        (1u64 << 20, 0, [(256, 8), (64, 1), (0, 0), (0, 0)]),
        (0, -1, [(256, 8), (64, 1), (0, 0), (0, 0)]),
        (u64::MAX, i32::MAX, [(256, 8), (64, 1), (0, 0), (0, 0)]),
    ];

    let mut checked = 0u64;
    let mut mesh_effect = 0u64;
    for &(bw, bh) in &[(8i32, 8i32), (16, 16), (32, 32), (16, 8), (8, 32)] {
        let bsize = bsize_of(bw, bh);
        for iter in 0..40 {
            let bh_mi = bh / MI;
            let bw_mi = bw / MI;
            let mi_row = (bh_mi + rng.below((mi_rows - 2 * bh_mi).max(1) as u64) as i32) / bh_mi * bh_mi;
            let mi_col = rng.below((mi_cols - bw_mi).max(1) as u64) as i32 / bw_mi * bw_mi;
            let dir = if rng.below(2) == 0 {
                intrabc::IntrabcMotionDirection::Above
            } else {
                intrabc::IntrabcMotionDirection::Left
            };
            let dv_ref = intrabc::find_ref_dv(tile, 16, mi_row);
            let Some(limits) = narrowed_limits(dir, tile, mi_row, mi_col, bw, bh, 16, 4, dv_ref) else {
                continue;
            };
            let block = (mi_col * MI, mi_row * MI);
            let epb = [63, 800 >> 6, 40000 >> 6][(iter % 3) as usize];
            let costs = make_costs(&mut rng, epb);
            let sadpb = [20, 35, 60][(iter % 3) as usize];

            let mut winners = Vec::new();
            for (ci, &(thresh, mv_diff_th, patterns)) in mesh_cfgs.iter().enumerate() {
                let mut ctrls = intrabc::IbcCtrls::for_level(3);
                ctrls.exhaustive_mesh_thresh = thresh;
                ctrls.mesh_search_mv_diff_threshold = mv_diff_th;
                for (slot, &(r, i)) in ctrls.mesh_patterns.iter_mut().zip(patterns.iter()) {
                    *slot = intrabc::MeshPattern { range: r, interval: i };
                }
                let (rs_x, rs_y, _var) = intrabc::full_pixel_search(
                    &pic,
                    stride,
                    block,
                    bw as usize,
                    bh as usize,
                    &cfg,
                    limits,
                    dv_ref,
                    sadpb,
                    costs.errorperbit,
                    &ctrls,
                    (bw as u32).trailing_zeros() - 2,
                    (bh as u32).trailing_zeros() - 2,
                    &costs.port,
                    false,
                );
                let (c_x, c_y) = cref::full_pixel_search(
                    &pic,
                    stride,
                    block,
                    bsize,
                    (i32::from(dv_ref.x), i32::from(dv_ref.y)),
                    sadpb,
                    (limits.col_min, limits.col_max, limits.row_min, limits.row_max),
                    thresh,
                    mv_diff_th,
                    &patterns,
                    &costs.as_search_costs(false),
                );
                assert_eq!(
                    (rs_x, rs_y),
                    (c_x, c_y),
                    "full_pixel_search diverges {bw}x{bh} mi=({mi_row},{mi_col}) dir={dir:?} cfg={ci} iter={iter}"
                );
                winners.push((c_x, c_y));
                checked += 1;
            }
            // Anti-vacuity: forced-mesh vs mesh-killed winners must differ
            // for at least SOME case (proves the mesh integrates + fires).
            if winners[1] != winners[2] {
                mesh_effect += 1;
            }
        }
    }
    assert!(checked > 250, "too few full-search cases: {checked}");
    assert!(mesh_effect > 0, "mesh never changed the winner: mesh path vacuous");
}

// ---------------------------------------------------------------------------
// intrabc_hash_search end-to-end (chunk-4 table feeding the chunk-5 search)
// ---------------------------------------------------------------------------

#[test]
fn c_parity_intrabc_hash_search() {
    let mut rng = Rng(0x1BC5_EA2C_0004);
    let (w, h, stride) = (640usize, 192usize, 640usize);
    let pic = screen_frame(&mut rng, w, h, stride);
    let (mi_rows, mi_cols) = ((h as i32) / MI, (w as i32) / MI);
    let tile = intrabc::TileMiBounds { mi_col_start: 0, mi_col_end: mi_cols, mi_row_start: 0, mi_row_end: mi_rows };
    let (max_hash, max_cand) = (64u8, 256u16);

    // Port table + C table (chunk-4-locked equal; both built fresh here).
    let rs_table = intrabc_hash::generate_ibc_data(&pic, stride, w, h, max_hash, max_cand, false);
    let mut c_table = cref::CHashTable::new();
    {
        let mut vals = [cref::generate_block_2x2_hash(&pic, stride, w, h), vec![0u32; w * h]];
        let mut src_idx = 0usize;
        let mut size = 4usize;
        while size <= usize::from(max_hash) {
            let dst_idx = 1 - src_idx;
            vals[dst_idx] = cref::generate_block_hash(w, h, size, &vals[src_idx]);
            c_table.add(&vals[dst_idx], w, h, size, max_cand);
            src_idx = dst_idx;
            size <<= 1;
        }
    }

    let costs = make_costs(&mut rng, 63);
    let sc = costs.as_search_costs(false);
    let mut bufs = intrabc_hash::BlockHashBuffers::new();

    let mut hits = 0u64;
    let mut misses = 0u64;
    let mut rejected_by_validity = 0u64;
    let mut checked = 0u64;
    for &bs in &[8i32, 16, 32, 64] {
        let bsize = bsize_of(bs, bs);
        let b_mi = bs / MI;
        for _ in 0..80 {
            let mi_row = (rng.below((mi_rows - b_mi).max(1) as u64) as i32) / b_mi * b_mi;
            let mi_col = (rng.below((mi_cols - b_mi).max(1) as u64) as i32) / b_mi * b_mi;
            let dir = if rng.below(2) == 0 {
                intrabc::IntrabcMotionDirection::Above
            } else {
                intrabc::IntrabcMotionDirection::Left
            };
            let dv_ref = intrabc::find_ref_dv(tile, 16, mi_row);
            let Some(limits) = narrowed_limits(dir, tile, mi_row, mi_col, bs, bs, 16, 4, dv_ref) else {
                continue;
            };
            let (x_pos, y_pos) = (mi_col * MI, mi_row * MI);

            // Port: query + bucket + selection.
            let (hv1, hv2) =
                intrabc_hash::get_block_hash_value(&pic[y_pos as usize * stride + x_pos as usize..], stride, bs as usize, &mut bufs);
            let rs = intrabc::hash_search_best_in_bucket(
                &rs_table
                    .bucket(hv1)
                    .iter()
                    .map(|e| intrabc::BlockHashEntry { x: i32::from(e.x), y: i32::from(e.y), hash_value2: e.hash_value2 })
                    .collect::<Vec<_>>(),
                hv2,
                x_pos,
                y_pos,
                mi_row,
                mi_col,
                bs,
                bs,
                b_mi,
                b_mi,
                tile,
                4,
                64,
                limits,
                &pic,
                stride,
                dv_ref,
                &costs.port,
                costs.errorperbit,
                false,
            );

            // C: the whole exported hash search.
            let c = cref::intrabc_hash_search(
                &pic,
                stride,
                (x_pos, y_pos),
                bsize,
                (i32::from(dv_ref.x), i32::from(dv_ref.y)),
                &c_table,
                max_hash,
                4,
                (0, mi_rows, 0, mi_cols),
                (limits.col_min, limits.col_max, limits.row_min, limits.row_max),
                &sc,
            );

            match (rs, c) {
                (None, None) => {
                    misses += 1;
                    // Distinguish "no identical content" from "candidates
                    // existed but every one was rejected" (dv-validity /
                    // in-bounds) — both must agree, and we want both kinds.
                    if rs_table.count(hv1) > 1 {
                        rejected_by_validity += 1;
                    }
                }
                (Some((dv, cost)), Some((cx, cy, ccost))) => {
                    assert_eq!(
                        (i32::from(dv.x), i32::from(dv.y), cost),
                        (cx * 8, cy * 8, ccost),
                        "hash search winner diverges at mi=({mi_row},{mi_col}) bs={bs} dir={dir:?}"
                    );
                    hits += 1;
                }
                (rs, c) => panic!(
                    "hash search hit/miss disagrees at mi=({mi_row},{mi_col}) bs={bs} dir={dir:?}: port={rs:?} C={c:?}"
                ),
            }
            checked += 1;
        }
    }
    assert!(checked > 120, "too few hash-search cases: {checked}");
    assert!(hits > 20, "no hash hits: fixture has no repeats in range ({hits})");
    assert!(misses > 5, "no hash misses ({misses})");
    assert!(
        rejected_by_validity > 0,
        "no populated-bucket misses: dv-validity rejection untested"
    );
}

// ---------------------------------------------------------------------------
// The whole per-block driver (both directions, either/or hash gate)
// ---------------------------------------------------------------------------

#[test]
fn c_parity_intra_bc_search_driver() {
    let mut rng = Rng(0x1BC5_EA2C_0005);
    let (w, h, stride) = (640usize, 192usize, 640usize);
    let pic = screen_frame(&mut rng, w, h, stride);
    let (mi_rows, mi_cols) = ((h as i32) / MI, (w as i32) / MI);
    let tile = intrabc::TileMiBounds { mi_col_start: 0, mi_col_end: mi_cols, mi_row_start: 0, mi_row_end: mi_rows };
    let (max_hash, max_cand) = (64u8, 256u16);

    let rs_table = intrabc_hash::generate_ibc_data(&pic, stride, w, h, max_hash, max_cand, false);
    let mut c_table = cref::CHashTable::new();
    {
        let mut vals = [cref::generate_block_2x2_hash(&pic, stride, w, h), vec![0u32; w * h]];
        let mut src_idx = 0usize;
        let mut size = 4usize;
        while size <= usize::from(max_hash) {
            let dst_idx = 1 - src_idx;
            vals[dst_idx] = cref::generate_block_hash(w, h, size, &vals[src_idx]);
            c_table.add(&vals[dst_idx], w, h, size, max_cand);
            src_idx = dst_idx;
            size <<= 1;
        }
    }

    let mut bufs = intrabc_hash::BlockHashBuffers::new();
    let mut checked = 0u64;
    let mut two_cand = 0u64;
    let mut hash_path = 0u64;
    let mut pixel_path = 0u64;
    let mut nonempty = 0u64;

    // Level-3 ctrls (the M0 allintra preset) + the level-5 hash-8 shape.
    for &(level, hash_on) in &[(3u8, true), (5, true), (3, false)] {
        let mut ctrls = intrabc::IbcCtrls::for_level(level);
        if !hash_on {
            ctrls.max_block_size_hash = 0; // C's "hash disabled" encoding
        }
        for &(bw, bh) in &[(8i32, 8i32), (16, 16), (32, 32), (16, 8), (8, 16)] {
            let bsize = bsize_of(bw, bh);
            let bh_mi = bh / MI;
            let bw_mi = bw / MI;
            for iter in 0..30 {
                let mi_row = (rng.below((mi_rows - bh_mi).max(1) as u64) as i32) / bh_mi * bh_mi;
                let mi_col = (rng.below((mi_cols - bw_mi).max(1) as u64) as i32) / bw_mi * bw_mi;
                let costs = make_costs(&mut rng, [63, 12, 400][(iter % 3) as usize]);
                let sadpb = [20, 35, 60][(iter % 3) as usize];
                // dv_ref: the fallback default (the pre-chunk-6 shape) or a
                // random whole-pel DV (as a from-stack value would be).
                let dv_ref = if rng.below(2) == 0 {
                    intrabc::find_ref_dv(tile, 16, mi_row)
                } else {
                    Mv {
                        x: ((rng.below(64) as i32 - 48) * 8) as i16,
                        y: ((rng.below(48) as i32 - 40) * 8) as i16,
                    }
                };

                // Port: bucket for this block (same bucket both directions).
                let hash_eligible = bw == bh && bw <= i32::from(ctrls.max_block_size_hash);
                let (bucket_entries, hv2) = if hash_eligible {
                    let (hv1, hv2) = intrabc_hash::get_block_hash_value(
                        &pic[(mi_row * MI) as usize * stride + (mi_col * MI) as usize..],
                        stride,
                        bw as usize,
                        &mut bufs,
                    );
                    (
                        rs_table
                            .bucket(hv1)
                            .iter()
                            .map(|e| intrabc::BlockHashEntry {
                                x: i32::from(e.x),
                                y: i32::from(e.y),
                                hash_value2: e.hash_value2,
                            })
                            .collect::<Vec<_>>(),
                        hv2,
                    )
                } else {
                    (Vec::new(), 0)
                };
                let buckets: [Option<&[intrabc::BlockHashEntry]>; 2] = if hash_eligible {
                    [Some(&bucket_entries), Some(&bucket_entries)]
                } else {
                    [None, None]
                };

                let rs = intrabc::intra_bc_search(
                    &pic,
                    stride,
                    bw,
                    bh,
                    bw_mi,
                    bh_mi,
                    mi_row,
                    mi_col,
                    mi_rows,
                    mi_cols,
                    16,
                    4,
                    64,
                    tile,
                    dv_ref,
                    &intrabc::init_search_sites(stride),
                    &ctrls,
                    sadpb,
                    costs.errorperbit,
                    false,
                    &costs.port,
                    buckets,
                    hv2,
                );

                let c = cref::intra_bc_search_driver(
                    &pic,
                    stride,
                    bsize,
                    (bw, bh),
                    (mi_row, mi_col),
                    (mi_rows, mi_cols),
                    (16, 4),
                    (0, mi_rows, 0, mi_cols),
                    (i32::from(dv_ref.x), i32::from(dv_ref.y)),
                    ctrls.search_dir,
                    ctrls.max_block_size_hash,
                    ctrls.exhaustive_mesh_thresh,
                    ctrls.mesh_search_mv_diff_threshold,
                    &[
                        (ctrls.mesh_patterns[0].range, ctrls.mesh_patterns[0].interval),
                        (ctrls.mesh_patterns[1].range, ctrls.mesh_patterns[1].interval),
                        (ctrls.mesh_patterns[2].range, ctrls.mesh_patterns[2].interval),
                        (ctrls.mesh_patterns[3].range, ctrls.mesh_patterns[3].interval),
                    ],
                    if hash_on { Some(&c_table) } else { None },
                    sadpb,
                    &costs.as_search_costs(false),
                );

                let rs_pairs: Vec<(i16, i16)> = rs.iter().map(|m| (m.x, m.y)).collect();
                assert_eq!(
                    rs_pairs, c,
                    "driver diverges level={level} hash={hash_on} {bw}x{bh} mi=({mi_row},{mi_col}) iter={iter}"
                );
                checked += 1;
                if c.len() == 2 {
                    two_cand += 1;
                }
                if !c.is_empty() {
                    nonempty += 1;
                    // Classify the first candidate's path: a hash hit means
                    // the port short-circuited (observable via the bucket).
                    if hash_eligible && !bucket_entries.is_empty() {
                        hash_path += 1;
                    } else {
                        pixel_path += 1;
                    }
                }
            }
        }
    }
    assert!(checked > 250, "too few driver cases: {checked}");
    assert!(nonempty > 60, "driver produced too few candidates: vacuous ({nonempty})");
    assert!(two_cand > 20, "both-directions arm untested ({two_cand})");
    assert!(hash_path > 20 && pixel_path > 20, "one search path untested (hash={hash_path} pixel={pixel_path})");
}
