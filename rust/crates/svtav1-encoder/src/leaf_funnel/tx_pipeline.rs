//! TX pipeline for one transform unit (u8 + the bd10 chain).
//!
//! Split out of `leaf_funnel.rs` on 2026-08-16 (11,247 lines).
//! PURE CODE MOVEMENT: every item keeps its name, order and effective
//! visibility (file-private became `pub(super)`, the same scope).

use super::*;

// ---------------------------------------------------------------------------
// TX pipeline for one transform unit
// ---------------------------------------------------------------------------

/// How the caller consumes [`TxUnitOut::bits`] — C's coefficient-rate tier for
/// this call site.
///
/// C's rate tiers are an `if / else if / else` and the real estimator is NEVER
/// called when a closed form applies (`product_coding_loop.c:4914-4934`, and
/// identically `:5540-5564`, `:5883-5890`). This enum carries that structure to
/// the one place that can act on it. It is NOT a "this value is dead" hint: on
/// [`RateMode::Lvl0Closed`] `bits` is still exactly the number the consumer
/// uses, it is just produced by the closed form directly instead of by
/// [`cost_coeffs_txb`] and then overwritten.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum RateMode {
    /// The real per-coefficient entropy cost (C `svt_aom_txb_estimate_coeff_bits`).
    Exact,
    /// C `coeff_rate_est_lvl == 0` on the `perform_tx_partitioning` path:
    /// `th = (txw*txh) >> 6; eob < th ? 6000 + eob*1000 : 3000 + eob*100`
    /// (`product_coding_loop.c:5540-5564`). The port used to compute the exact
    /// rate inside `tx_unit` and then discard it in the depth loop; computing
    /// the closed form here instead is the SAME ARITHMETIC, so no deadness
    /// argument is involved.
    Lvl0Closed,
}

pub(super) struct TxUnitOut {
    pub(super) eob: u16,
    /// Packed (32-capped) quantized levels.
    pub(super) qcoeff: Vec<i32>,
    /// Reconstructed pixels (w x h raster).
    ///
    /// EMPTY when the call passed `need_recon == false` — C skips the inverse
    /// transform entirely at those stages (`product_coding_loop.c:4783-4784`).
    /// Empty rather than zeroed deliberately: a caller that starts reading it
    /// gets an index panic, never silently wrong pixels.
    pub(super) recon: Vec<u8>,
    /// Frequency-domain RESIDUAL distortion (MDS1 path) or spatial SSE
    /// << 4 (MDS3 path), already shifted like C.
    pub(super) dist: u64,
    /// Coefficient bits (or skip-txb bits when eob == 0).
    pub(super) bits: i32,
    /// `(dc_sign << 6) | min(cul_level, 63)` neighbor byte.
    pub(super) cul: u8,
}

impl TxUnitOut {
    /// The no-chroma placeholder for `has_uv == 0` blocks: C never runs
    /// the chroma full loop there, so every chroma term is EXACTLY zero
    /// (no skip-txb rate either — the syntax doesn't exist).
    pub(super) fn absent() -> Self {
        TxUnitOut {
            eob: 0,
            qcoeff: Vec::new(),
            recon: Vec::new(),
            dist: 0,
            bits: 0,
            cul: 0,
        }
    }
}

/// C's per-tx-type OUTPUT slots, `ctx->quant_coeff_ptr[txt_itr]` and
/// `ctx->recon_ptr[txt_itr]` (`md_process.c:585-600`).
///
/// C allocates them ONCE per encoder thread, in
/// `svt_aom_mode_decision_context_ctor` (`md_process.c:214`, reached from
/// `svt_aom_enc_dec_context_ctor`'s `EB_NEW` at `enc_dec_process.c:108`), as
/// three `TX_TYPES`-deep picture-buffer pools, and reuses them for every
/// tx-type trial of every transform unit of every block of every frame — which
/// is why C's whole-encode allocator traffic is ~2,600 calls at every preset
/// and every content. `tx_type_search` writes trial `txt_itr` into slot
/// `txt_itr` and remembers `best_tx_type`; nothing is copied and nothing is
/// freed.
///
/// This is the port's equivalent: the caller owns the buffers, [`tx_unit_inner`]
/// writes into them, and the winner is materialised into an owned `Vec` exactly
/// once per transform unit — where C hands out its pooled `coeff_tmp` slot.
///
/// NEITHER BUFFER IS RE-ZEROED, and that is the point (a recycled buffer that
/// is re-zeroed pays a `memset` that `vec![0; n]`'s `calloc` gets for free —
/// `700357e2`'s finding from the other direction). It is sound because every
/// position is written before it is read on every path:
///   * `qcoeff`: `quant_coding::{fp_one, b_one}` assign `qcoeff[rc]` on BOTH
///     arms of their `if tmp32 != 0`, for `rc` in `0..min(coeffs.len(),
///     qcoeff.len(), dqcoeff.len())`, and `packed` is exactly `pw * ph`; the
///     two QM paths open with their own `qcoeff[..coeffs.len()].fill(0)`
///     (`qm.rs:134`/`:195`). `optimize_b` and `perform_noise_normalization`
///     only revise positions the quantizer already wrote.
///   * `recon`: `residual::recon_add_clamp` writes every `h * w`, the
///     `eob == 0` arm copies the prediction over every row, and the
///     coded-lossless arm zips all 16.
#[derive(Default)]
pub(super) struct TxOutBufs {
    /// C `ctx->quant_coeff_ptr[txt_itr]`.
    pub(super) qcoeff: Vec<i32>,
    /// C `ctx->recon_ptr[txt_itr]`.
    pub(super) recon: Vec<u8>,
}

/// What [`tx_unit_inner`] returns when its outputs live in a caller-owned
/// [`TxOutBufs`]: the scalars, plus the VALID PREFIX LENGTH of each buffer.
///
/// The lengths are explicit because a pooled buffer keeps the largest capacity
/// it has ever been grown to, so `bufs.qcoeff.len()` is NOT this unit's
/// `pw * ph`. `recon_len == 0` reproduces [`TxUnitOut::recon`]'s empty-when-
/// skipped contract exactly.
pub(super) struct TxUnitMeta {
    pub(super) eob: u16,
    pub(super) q_len: usize,
    pub(super) recon_len: usize,
    pub(super) dist: u64,
    pub(super) bits: i32,
    pub(super) cul: u8,
}

/// Grow `buf` to at least `n` and hand back the first `n` — WITHOUT zeroing a
/// buffer that already exists. The [`TxOutBufs`] twin of [`TxScratch::grown`];
/// see that doc for why the re-zero is the thing being removed and
/// [`TxOutBufs`] for the per-path proof that every returned position is written
/// before it is read.
///
/// An EMPTY buffer is filled with `vec![0; n]` — the exact expression, at the
/// exact program point, that this call site used before the buffers were lifted
/// out of [`tx_unit_inner`]. That matters and is measured: a caller that hands
/// in an empty `TxOutBufs` (the owned-output wrapper, and the one-candidate arm
/// of `txt_search`) must allocate the SAME size class from the SAME allocator
/// entry point at the SAME point in the transform pipeline, or the live-set
/// interleaving changes — pre-sizing those two buffers in the CALLER instead,
/// which is otherwise identical arithmetic, moved the aarch64 inter peak RSS at
/// 2048 by +4.4 % (interleaved `mem_bisect.sh`, 21 rounds).
#[inline]
fn grown_out<T: Copy + Default>(buf: &mut Vec<T>, n: usize) -> &mut [T] {
    if buf.is_empty() {
        *buf = alloc::vec![T::default(); n];
    } else if buf.len() < n {
        buf.resize(n, T::default());
    }
    &mut buf[..n]
}

/// Reusable per-thread scratch for [`tx_unit`]'s five purely-internal buffers.
///
/// `tx_unit` used to heap-allocate seven `Vec`s per transform unit, and it runs
/// once per (candidate x tx type x tx unit) — the hottest allocation site in the
/// encoder. Profiling the port at 512x512 measured the malloc/free family at
/// **5.5 % of preset-6 self time, 6.7 % at preset 10 and ~9.9 % at preset 2**,
/// against 0.01–0.15 ms on the C side, which allocates none of this per TU.
///
/// SIX of the seven never escape the function (`residual`, `coeffs`, `packed`,
/// `dq_full`, `inv`, `dqcoeff`) and are hoisted here. The remaining two
/// (`qcoeff`, `recon`) are moved into the returned [`TxUnitOut`] and still
/// allocate.
///
/// **`dqcoeff` was the miscounted one** (this comment said "five of seven" and
/// "the remaining two" while the body allocated three). It is written by the
/// quantizer, read by the `dq_full` fold and the frequency-domain SSE, and
/// returned to nobody. On a 512x512 video-mode key frame at preset 8,
/// `tx_unit_inner` was the single largest allocation SITE in the whole encoder
/// — 72,919 calls to the allocator, measured by heaptrack on r7900x — and this
/// is a third of them.
///
/// Byte-identity argument, buffer by buffer — the whole point is that a reused
/// buffer must not leak a previous TU's bytes into this one:
/// * `residual` and `packed` were `Vec::with_capacity` + a loop that pushes
///   EVERY element, so they were never zero-initialised to begin with; here they
///   are `clear()`ed and refilled the same way. Same values, same order.
/// * `coeffs`, `dq_full` and `inv` were `vec![0; n]`; here they are resized and
///   explicitly `fill(0)`ed over the working length before use. Same bytes.
///   (`dq_full` in particular is only partially overwritten — the fold copies a
///   `pw x ph` corner into a `w x h` buffer — so its zeroing is load-bearing and
///   is kept verbatim rather than reasoned away.)
///
/// Sizes are bounded by the largest AV1 transform, 64x64, so the scratch tops out
/// at 4096 i32 per full-size buffer and never reallocates after warmup.
#[derive(Default)]
pub(super) struct TxScratch {
    pub(super) residual: Vec<i32>,
    pub(super) coeffs: Vec<i32>,
    pub(super) packed: Vec<i32>,
    pub(super) dq_full: Vec<i32>,
    pub(super) inv: Vec<i32>,
    pub(super) dqcoeff: Vec<i32>,
}

impl TxScratch {
    /// Resize `buf` to `n` and zero it — the exact replacement for a
    /// `vec![0; n]` that the reused-buffer version must reproduce byte for byte.
    #[inline]
    pub(super) fn zeroed(buf: &mut Vec<i32>, n: usize) -> &mut [i32] {
        if buf.len() < n {
            buf.resize(n, 0);
        }
        let s = &mut buf[..n];
        s.fill(0);
        s
    }

    /// Resize `buf` to at least `n` and hand back the first `n` — WITHOUT
    /// zeroing, for a buffer whose every element is written before it is read.
    ///
    /// The `s.fill(0)` in [`TxScratch::zeroed`] is a real `memset` of up to
    /// 4 KiB per transform unit, and `tx_unit` is the hottest allocation and
    /// memory-traffic site in the funnel — the #1 caller of the
    /// `memset`/`memmove`/`memcpy` family on the still arm, 192 of 753 self
    /// samples (25.5 %, 5.61 ms of a 552.9 ms frame at gradient 512x512
    /// preset 2, `tools/perf_profile/ancestor.py`). `vec![0; n]` gets its zeros
    /// from fresh calloc pages for free; an explicit re-zero of a recycled
    /// buffer does not (`700357e2`'s finding, from the other direction).
    ///
    /// USE THIS ONLY WHERE THE FULL-WRITE IS PROVEN, and say where. It is NOT
    /// interchangeable with [`TxScratch::zeroed`]: `dq_full`'s zero-fill IS
    /// load-bearing on the 64-dim shapes, where only the `pw x ph` corner is
    /// written.
    #[inline]
    pub(super) fn grown(buf: &mut Vec<i32>, n: usize) -> &mut [i32] {
        if buf.len() < n {
            buf.resize(n, 0);
        }
        &mut buf[..n]
    }
}

/// The per-thread state one `txt_search` needs: the CURRENT trial's output
/// slot, the BEST trial's, and the one residual every trial transforms.
///
/// C keeps `TX_TYPES` output slots (`ctx->quant_coeff_ptr[]` / `recon_ptr[]`,
/// `md_process.c:585-600`) because it indexes by `txt_itr`; the port only ever
/// needs the current trial and the best so far, so two suffice and a
/// `mem::swap` on a new best does what C's `best_tx_type` index does. The
/// residual is C's `cand_bf->residual->y_buffer`, which
/// `perform_tx_partitioning` fills ONCE per (tx-depth, TXB) at
/// `product_coding_loop.c:5336` and every tx-type trial then transforms
/// (`svt_aom_estimate_transform` reads it at `:4730`).
#[derive(Default)]
pub(super) struct TxtScratch {
    pub(super) cur: TxOutBufs,
    pub(super) bst: TxOutBufs,
    pub(super) residual: Vec<i32>,
}

#[cfg(feature = "std")]
std::thread_local! {
    static TXT_OUT: core::cell::RefCell<TxtScratch> =
        const { core::cell::RefCell::new(TxtScratch {
            cur: TxOutBufs { qcoeff: Vec::new(), recon: Vec::new() },
            bst: TxOutBufs { qcoeff: Vec::new(), recon: Vec::new() },
            residual: Vec::new(),
        }) };
}

/// Run `f` over the per-thread [`TxtScratch`].
///
/// `try_borrow_mut` + closure hand-back, the same contract as
/// [`tx_unit_screened_into`]'s `TX_SCRATCH` borrow: a re-entrant caller gets
/// fresh buffers rather than a panic.
pub(super) fn with_txt_out<R>(f: impl FnOnce(&mut TxtScratch) -> R) -> R {
    #[cfg(feature = "std")]
    {
        let taken = TXT_OUT.with(move |cell| match cell.try_borrow_mut() {
            Ok(mut sc) => Ok(f(&mut sc)),
            Err(_) => Err(f),
        });
        match taken {
            Ok(r) => r,
            Err(f) => f(&mut TxtScratch::default()),
        }
    }
    #[cfg(not(feature = "std"))]
    {
        f(&mut TxtScratch::default())
    }
}

#[cfg(feature = "std")]
std::thread_local! {
    static TX_SCRATCH: core::cell::RefCell<TxScratch> =
        const { core::cell::RefCell::new(TxScratch {
            residual: Vec::new(), coeffs: Vec::new(), packed: Vec::new(),
            dq_full: Vec::new(), inv: Vec::new(), dqcoeff: Vec::new(),
        }) };
}

/// C `svt_av1_compute_cul_level` (full_loop.c:1356).
pub(super) fn compute_cul_level(scan: &[u16], qcoeff: &[i32], eob: u16) -> u8 {
    let mut cul: u32 = 0;
    for c in 0..eob as usize {
        cul += qcoeff[scan[c] as usize].unsigned_abs();
        if cul >= 63 {
            break;
        }
    }
    cul = cul.min(63);
    let dc = if eob > 0 { qcoeff[0] } else { 0 };
    if dc < 0 {
        cul |= 1 << 6;
    } else if dc > 0 {
        cul += 2 << 6;
    }
    cul as u8
}

/// C's coefficient-SATD early exit, carried across one `tx_type_search` loop.
///
/// C `tx_type_search`, `product_coding_loop.c:4741-4755`, verbatim:
///
/// ```text
/// if (satd_early_exit_th) {
///     int satd = svt_aom_satd(&tx_coeffs[txb_1d_offset], txbwidth * txbheight)
///                << ctx->mds_subres_step;
///     if (satd < best_satd_tx_search) { best_satd_tx_search = satd; }
///     else if ((satd - best_satd_tx_search) * 100
///              > best_satd_tx_search * satd_early_exit_th) { continue; }
///     ...
/// }
/// svt_aom_quantize_inv_quantize(...)   // survivors only
/// ```
///
/// `best` is a RUNNING minimum over the whole search (C initialises it once at
/// `:4643`, before the tx-type-group loop, not per group), so the admitted set
/// depends on candidate ORDER — which is why the screen is threaded through the
/// loop as one object rather than recomputed per call. DCT_DCT is always the
/// first candidate and always faces `best == i64::MAX`, so it is never rejected
/// and `dct_cost` (the rate gate's denominator) is always populated.
///
/// `<< ctx->mds_subres_step` is absent because the port has no subsampled-
/// residual mode (`mds_subres_step` is 0 at every preset it expresses); the
/// shift would scale both sides of every comparison identically anyway.
pub(super) struct SatdScreen {
    /// C `satd_early_exit_th`, already qp-scaled by the caller. The screen is
    /// only constructed when this is non-zero (C's `if (satd_early_exit_th)`).
    th: i64,
    /// C `best_satd_tx_search`.
    best: i64,
    /// The last trial's SATD and the `best` it faced BEFORE being folded in —
    /// for the `SVTAV1_TXT_XY` dump only, which prints the pre-update value.
    last: (i64, i64),
}

impl SatdScreen {
    /// C `best_satd_tx_search = INT_MAX` (`product_coding_loop.c:4643`).
    pub(super) fn new(th: i64) -> Self {
        SatdScreen {
            th,
            best: i64::MAX,
            last: (0, 0),
        }
    }

    /// The last trial's `(satd, best_before_update)`, for the dump.
    pub(super) fn last(&self) -> (i64, i64) {
        self.last
    }

    /// Fold one trial's coefficients in; `true` promotes it to quantize.
    ///
    /// C's SATD is an i32 sum (common_dsp_rtcd.c:48); this screen retains
    /// the port's wider i64 sum. Narrowing would change large-coefficient
    /// inputs, witnessed by the DSP reduction test's closed-form edge cases.
    fn admit(&mut self, coeffs: &[i32]) -> bool {
        #[cfg(target_arch = "x86_64")]
        let satd = svtav1_dsp::hadamard::sum_abs_coeffs_wide(coeffs);
        // Preserve the existing inlined reduction on other architectures.
        #[cfg(not(target_arch = "x86_64"))]
        let satd = {
            let mut satd: i64 = 0;
            for &c in coeffs.iter() {
                satd += c.unsigned_abs() as i64;
            }
            satd
        };
        self.last = (satd, self.best);
        if satd < self.best {
            self.best = satd;
            true
        } else {
            (satd - self.best) * 100 <= self.best * self.th
        }
    }
}

/// Forward transform + (optional RDOQ) quantize + inverse recon + dist +
/// coeff bits for one TX unit. Mirrors the DCT/TXT iteration body of
/// `tx_type_search` / `perform_dct_dct_tx` / `svt_aom_full_loop_uv`.
///
/// `spatial_dist`: MDS3 (recon vs source SSE << 4); else the MDS1
/// freq-domain path. `do_rdoq` follows C `mds_do_rdoq && rdoq enabled`.
///
/// `crop`: the cropped-TX distortion extent — C `cropped_tx_width` /
/// `cropped_tx_height` (product_coding_loop.c:4664, and the chroma
/// `_uv` twin at full_loop.c:2228), from `frame_geom::cropped_tx_dims`.
/// It clips ONLY the spatial distortion kernels (SSE / psy / tx-bias
/// facade) to the part of this TX block that is inside the ALIGNED
/// frame — never the residual, transform, quantizer, RDOQ, recon or
/// coefficient rate, all of which C runs over the FULL tx block. On a
/// 64-aligned frame `crop == (w, h)` and every expression below is
/// unchanged; only a partial-superblock straddle can make them differ.
///
/// `need_recon`: does the CALLER read [`TxUnitOut::recon`]? C gates its inverse
/// transform on `ctx->mds_do_spatial_sse || (!is_inter && cand->tx_depth)`
/// (`product_coding_loop.c:4783-4784`, chroma twin `full_loop.c:2313`), and the
/// all-intra derivation pins `spatial_sse_full_loop_level = 3`, so C's MDS1 and
/// MDS2 invert nothing at all. This is an EXPLICIT caller parameter, not
/// `spatial_dist` reused: `spatial_dist == false && need_recon == true` is a
/// legal combination the moment a depth-1 neighbour prediction needs the
/// reconstruction, and inferring it would make a future caller silently wrong.
/// Default `true`.
///
/// `rate_mode`: which of C's coefficient-rate tiers this call site consumes —
/// see [`RateMode`].
#[allow(clippy::too_many_arguments)]
pub(super) fn tx_unit(
    src: &[u8],
    src_stride: usize,
    src_off: usize,
    pred: &[u8],
    pred_stride: usize,
    pred_off: usize,
    w: usize,
    h: usize,
    tx_type: usize,
    plane_type: usize,
    txb_skip_ctx: usize,
    dc_sign_ctx: usize,
    intra_dir: usize,
    qt: &QuantTable,
    frame: &FunnelFrame,
    rates: &MdRates,
    do_rdoq: bool,
    spatial_dist: bool,
    crop: (usize, usize),
    need_recon: bool,
    rate_mode: RateMode,
) -> TxUnitOut {
    tx_unit_screened(
        src,
        src_stride,
        src_off,
        pred,
        pred_stride,
        pred_off,
        w,
        h,
        tx_type,
        plane_type,
        txb_skip_ctx,
        dc_sign_ctx,
        intra_dir,
        qt,
        frame,
        rates,
        do_rdoq,
        spatial_dist,
        crop,
        need_recon,
        rate_mode,
        None,
    )
    .expect("no screen was passed, so the unit is always committed")
}

/// [`tx_unit`] with C's SATD early exit wired into its middle.
///
/// C's `tx_type_search` (`product_coding_loop.c:4729-4776`) runs the tx-type
/// trial in TWO phases: the CHEAP one is the forward transform alone
/// (`svt_aom_estimate_transform`, `:4729`) plus a SATD over the coefficients it
/// just produced (`svt_aom_satd`, `:4742`); the EXPENSIVE one — quantize,
/// conditional RDOQ, inverse, distortion, coefficient rate
/// (`svt_aom_quantize_inv_quantize`, `:4757`, and everything after it) — runs
/// only for the trials the SATD admits (`:4753`'s `continue` skips the rest of
/// the loop body entirely). `screen` is that gate, evaluated at exactly C's
/// position in the pipeline; `None` restores the single-phase [`tx_unit`].
///
/// Returns `None` for a REJECTED trial — one C never quantizes. A rejected
/// trial contributes nothing on either side (C's `continue` is before every
/// write to `eob_txt` / `y_txb_coeff_bits_txt` / `best_cost_tx_search`), so
/// admitting it and discarding its result afterwards — which is what this port
/// did until 2026-09-04 — produces the same bytes and pays for the whole
/// expensive phase to do it. Measured on `benchmarks/callcount_2026-09-04`:
/// 488,414 committed trials against C's 270,415 at gradient 512x512 qp40
/// preset 2, one edge worth 45.20 % of the port's instruction count.
///
/// OWNED-OUTPUT form: allocates a fresh [`TxOutBufs`] for this one call and
/// moves it into the returned [`TxUnitOut`]. The tx-type SEARCH does not use
/// it — `txt_search` drives [`tx_unit_screened_into`] over two pooled buffers,
/// C's shape — but the ~19 % of call sites that consume the buffers by value
/// (chroma, CFL, MDS1, the CFL-alpha loop) do, and their allocation count is
/// unchanged.
#[allow(clippy::too_many_arguments)]
pub(super) fn tx_unit_screened(
    src: &[u8],
    src_stride: usize,
    src_off: usize,
    pred: &[u8],
    pred_stride: usize,
    pred_off: usize,
    w: usize,
    h: usize,
    tx_type: usize,
    plane_type: usize,
    txb_skip_ctx: usize,
    dc_sign_ctx: usize,
    intra_dir: usize,
    qt: &QuantTable,
    frame: &FunnelFrame,
    rates: &MdRates,
    do_rdoq: bool,
    spatial_dist: bool,
    crop: (usize, usize),
    need_recon: bool,
    rate_mode: RateMode,
    screen: Option<&mut SatdScreen>,
) -> Option<TxUnitOut> {
    // EMPTY, deliberately: `grown_out` then fills each buffer with the same
    // `vec![0; n]`, of the same size, at the same point in `tx_unit_inner` as
    // before this change — so an owned-output call site's allocation count,
    // sizes, allocator entry points AND live-set interleaving are unchanged.
    // Both cheaper alternatives were measured and rejected: `grown_out`'s
    // `resize` on an empty buffer cost +0.88 M `memset` calls and moved 2 M
    // allocations from `calloc` to `malloc`, and pre-sizing here instead moved
    // the aarch64 inter peak RSS at 2048 by +4.4 %.
    let mut bufs = TxOutBufs::default();
    let m = tx_unit_screened_into(
        src,
        src_stride,
        src_off,
        pred,
        pred_stride,
        pred_off,
        w,
        h,
        tx_type,
        plane_type,
        txb_skip_ctx,
        dc_sign_ctx,
        intra_dir,
        qt,
        frame,
        rates,
        do_rdoq,
        spatial_dist,
        crop,
        need_recon,
        rate_mode,
        screen,
        // The owned-output sites are single-shot: there is no tx-type loop
        // above them to share a residual across, so each derives its own,
        // exactly as C's `perform_dct_dct_tx` / `full_loop_uv` do.
        None,
        &mut bufs,
    )?;
    // `bufs` started empty, so each buffer was allocated exactly once, at its
    // final size: the `truncate`s are no-ops on this path and only exist so a
    // future caller that hands in a warm `TxOutBufs` still gets the exact
    // `TxUnitOut` contract (`qcoeff` of length `pw * ph`, `recon` of length
    // `w * h` or EMPTY).
    let mut qcoeff = bufs.qcoeff;
    qcoeff.truncate(m.q_len);
    let mut recon = bufs.recon;
    recon.truncate(m.recon_len);
    Some(TxUnitOut {
        eob: m.eob,
        qcoeff,
        recon,
        dist: m.dist,
        bits: m.bits,
        cul: m.cul,
    })
}

/// [`tx_unit_screened`] writing its two outputs into CALLER-OWNED buffers —
/// C's `ctx->quant_coeff_ptr[txt_itr]` / `ctx->recon_ptr[txt_itr]` shape. See
/// [`TxOutBufs`].
#[allow(clippy::too_many_arguments)]
pub(super) fn tx_unit_screened_into(
    src: &[u8],
    src_stride: usize,
    src_off: usize,
    pred: &[u8],
    pred_stride: usize,
    pred_off: usize,
    w: usize,
    h: usize,
    tx_type: usize,
    plane_type: usize,
    txb_skip_ctx: usize,
    dc_sign_ctx: usize,
    intra_dir: usize,
    qt: &QuantTable,
    frame: &FunnelFrame,
    rates: &MdRates,
    do_rdoq: bool,
    spatial_dist: bool,
    crop: (usize, usize),
    need_recon: bool,
    rate_mode: RateMode,
    screen: Option<&mut SatdScreen>,
    pre_residual: Option<&[i32]>,
    out: &mut TxOutBufs,
) -> Option<TxUnitMeta> {
    // Borrow the per-thread scratch (see [`TxScratch`]). `try_borrow_mut`
    // rather than `borrow_mut`: tx_unit calls only DSP / quant / rate kernels
    // and never re-enters itself, but a future re-entrant caller should get a
    // fresh scratch, not a panic.
    //
    // `Result<_, screen>`: on a failed borrow the `&mut SatdScreen` is handed
    // BACK out of the closure rather than dropped inside it, so the fallback
    // path below can still run the gate. Same contract as
    // `detect::with_satd_scratch`'s closure hand-back.
    #[cfg(feature = "std")]
    let screen = {
        let taken = TX_SCRATCH.with(|cell| match cell.try_borrow_mut() {
            Ok(mut sc) => Ok(tx_unit_inner(
                &mut sc,
                src,
                src_stride,
                src_off,
                pred,
                pred_stride,
                pred_off,
                w,
                h,
                tx_type,
                plane_type,
                txb_skip_ctx,
                dc_sign_ctx,
                intra_dir,
                qt,
                frame,
                rates,
                do_rdoq,
                spatial_dist,
                crop,
                need_recon,
                rate_mode,
                screen,
                pre_residual,
                out,
            )),
            Err(_) => Err(screen),
        });
        match taken {
            Ok(o) => return o,
            Err(screen) => screen,
        }
    };
    let mut sc = TxScratch::default();
    tx_unit_inner(
        &mut sc,
        src,
        src_stride,
        src_off,
        pred,
        pred_stride,
        pred_off,
        w,
        h,
        tx_type,
        plane_type,
        txb_skip_ctx,
        dc_sign_ctx,
        intra_dir,
        qt,
        frame,
        rates,
        do_rdoq,
        spatial_dist,
        crop,
        need_recon,
        rate_mode,
        screen,
        pre_residual,
        out,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn tx_unit_inner(
    sc: &mut TxScratch,
    src: &[u8],
    src_stride: usize,
    src_off: usize,
    pred: &[u8],
    pred_stride: usize,
    pred_off: usize,
    w: usize,
    h: usize,
    tx_type: usize,
    plane_type: usize,
    txb_skip_ctx: usize,
    dc_sign_ctx: usize,
    intra_dir: usize,
    qt: &QuantTable,
    frame: &FunnelFrame,
    rates: &MdRates,
    do_rdoq: bool,
    spatial_dist: bool,
    crop: (usize, usize),
    need_recon: bool,
    rate_mode: RateMode,
    screen: Option<&mut SatdScreen>,
    pre_residual: Option<&[i32]>,
    out: &mut TxOutBufs,
) -> Option<TxUnitMeta> {
    let (crop_w, crop_h) = crop;
    debug_assert!(crop_w <= w && crop_h <= h, "crop must clip, never extend");
    let n = w * h;
    let c_tx = cc::tx_size_from_dims(w, h);
    let rs_tx_type = TX_TYPE_FROM_C[tx_type];

    // Build directly (clear + push, into the reused scratch) rather than
    // `vec![0; n]` + full overwrite: every element is written below, so the
    // zero-fill was dead. This pushes exactly h*w = n values in row-major order
    // — byte-identical contents, no `calloc`/`memset`, and after warmup no
    // allocation either.
    let TxScratch {
        residual,
        coeffs,
        packed: packed_buf,
        dq_full,
        inv,
        dqcoeff: dqcoeff_buf,
    } = sc;
    // C derives the residual ONCE per (tx-depth, TXB) —
    // `perform_tx_partitioning`'s `svt_aom_residual_kernel` at
    // `product_coding_loop.c:5336`, into `cand_bf->residual->y_buffer` — and
    // every tx-type trial then transforms THAT buffer
    // (`svt_aom_estimate_transform` reads it at `:4730`; `tx_type_search` has
    // no edge into the residual kernel at all). `pre_residual` is that shared
    // buffer: `txt_search` fills it once before its group loop and hands the
    // same slice to every trial. `None` restores the per-call derivation, which
    // is what a single-shot call site (MDS1, chroma, CfL, the owned-output
    // wrapper) wants — C's `perform_dct_dct_tx` and `full_loop_uv` likewise
    // derive their own.
    let residual: &[i32] = match pre_residual {
        Some(r) => &r[..n],
        None => {
            if residual.len() < n {
                residual.resize(n, 0);
            }
            let r = &mut residual[..n];
            // Every element is written by the kernel, so no zero-fill is needed
            // and the reused buffer cannot leak a previous TU's values.
            svtav1_dsp::residual::residual_i32(
                &src[src_off..],
                src_stride,
                &pred[pred_off..],
                pred_stride,
                w,
                h,
                r,
            );
            &*r
        }
    };
    let coeffs = TxScratch::zeroed(coeffs, n);
    // Coded-lossless (issue #5): C `svt_av1_estimate_transform`
    // (transforms.c:3950-3963) takes the 4x4 Walsh-Hadamard instead of the
    // DCT when the segment is lossless AND the tx is TX_4X4 (larger sizes fall
    // through — they cannot occur here because `mimic_only_tx_4x4` forces
    // every 8x8 block to depth 1, but the guard is kept as C keeps it), and
    // stores the kernel's output TRANSPOSED: `coeff_buffer[(j << 2) + i] =
    // dst[(i << 2) + j]`. The lossless tx type is always DCT_DCT (asserted
    // in C; the injection filter and `txt_on = false` guarantee it here).
    let lossless_wht = frame.coded_lossless && w == 4 && h == 4;
    if lossless_wht {
        debug_assert_eq!(tx_type, cc::DCT_DCT, "lossless txb must be DCT_DCT");
        let mut res16 = [0i16; 16];
        for (d, &s) in res16.iter_mut().zip(residual.iter()) {
            *d = s as i16;
        }
        let mut dst = [0i32; 16];
        svtav1_dsp::fwd_txfm::fwht4x4(&res16, &mut dst, 4);
        for i in 0..4 {
            for j in 0..4 {
                coeffs[(j << 2) + i] = dst[(i << 2) + j];
            }
        }
    } else {
        let ok = svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(
            residual,
            coeffs,
            w,
            TX_SIZE_FROM_C[c_tx],
            rs_tx_type,
        );
        debug_assert!(ok, "fwd txfm {w}x{h} type {tx_type}");
    }

    // ---- C's SATD early exit, at C's position: after the forward transform,
    // BEFORE the quantizer (product_coding_loop.c:4741-4755 sits between
    // `svt_aom_estimate_transform` at :4729 and `svt_aom_quantize_inv_quantize`
    // at :4757). A rejected trial returns here having paid only the residual
    // and the transform, exactly as C's `continue` does.
    //
    // Placed before the 64-dim fold to match C's ORDER; the two are
    // order-independent in practice because the fold only exists for w or h
    // > 32, and `search_dct_dct_only` forces `only_dct_dct` — hence
    // `satd_early_exit_th = 0` and no screen at all — on exactly those shapes.
    //
    // Placed AFTER the coded-lossless Walsh-Hadamard branch for the same
    // reason C's is after `svt_aom_estimate_transform`: C's SATD reads
    // whatever that call wrote, WHT included. The old post-hoc screen read a
    // DCT of the same residual instead. Both are unreachable — lossless
    // implies `txt_on == false` implies `only_dct` implies no screen — but
    // this position is the faithful one.
    if let Some(screen) = screen
        && !screen.admit(&coeffs[..n])
    {
        return None;
    }

    // 64-dim fold (svt_handle_transform64x64) + energy of discarded coeffs.
    let mut three_quad_energy: u64 = 0;
    let (pw, ph) = (w.min(32), h.min(32));
    let packed: &[i32] = if w > 32 || h > 32 {
        if w == 64 && h == 64 {
            three_quad_energy = energy_region(&coeffs[32..], 64, 32, 32)
                + energy_region(&coeffs[32 * 64..], 64, 64, 32);
        } else if w == 64 {
            // 64x32 / 64x16: top-right (w-32)-wide, h-tall region
            // (svt_handle_transform64x32_c / 64x16_c, transforms.c:3223).
            three_quad_energy = energy_region(&coeffs[32..], 64, 32, h.min(32));
        } else {
            // 32x64 / 16x64: bottom w-wide, (h-32)-tall region.
            three_quad_energy = energy_region(&coeffs[32 * w..], w, w, h - 32);
        }
        // Clear + extend rather than `vec![0; pw*ph]` + full copy: the loop
        // copies every one of the pw*ph elements, so the zero-fill was dead.
        // Byte-identical contents (same pw-wide rows in order).
        packed_buf.clear();
        packed_buf.reserve(pw * ph);
        for r in 0..ph {
            packed_buf.extend_from_slice(&coeffs[r * w..r * w + pw]);
        }
        &packed_buf[..pw * ph]
    } else {
        // No 64-dim fold: the packed coefficients ARE `coeffs` (pw*ph == n), so
        // borrow instead of cloning. `packed` is read-only from here on — the
        // quantizer writes qcoeff/dqcoeff, never its input — which is what makes
        // dropping the copy byte-inert.
        &coeffs[..n]
    };

    let scan = crate::entropy::scan_tables::scan(
        c_tx,
        crate::entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[tx_type] as usize,
    );
    let log_scale = TX_SCALE_TAB[c_tx];
    // [SVT_HDR_MODE] QM slices for this txb (2D transforms only; U and V
    // share the chroma table class, the LEVEL is plane-selected by the
    // caller via `qt.qm_level`).
    let qm = if tx_type < 9 && qt.qm_level < 15 {
        crate::qm::qm_slices(usize::from(qt.qm_level), plane_type == 1, c_tx)
    } else {
        None
    };
    // GROWN, NOT ZEROED — C's `ctx->quant_coeff_ptr[txt_itr]` slot (see
    // [`TxOutBufs`]), which C allocates once per encoder thread and never
    // clears between trials. The same full-write proof the `dqcoeff` comment
    // below gives for the dequantized twin covers this buffer: every one of the
    // four quantizer paths defines all `pw * ph` positions before anything
    // reads them.
    let qcoeff: &mut [i32] = grown_out(&mut out.qcoeff, pw * ph);
    // GROWN, NOT ZEROED. Only `pw * ph` of this is ever indexed, and all four
    // quantizer paths write EVERY one of those positions before anything reads
    // them, so the re-zero this used to do was dead:
    //   * `quant::quantize_fp` / `quantize_b` reach
    //     `quant_coding::quantize_{fp,b}_raster`, whose cores loop
    //     `for rc in 0..min(coeffs.len(), qcoeff.len(), dqcoeff.len())` and
    //     write both outputs at every position — and `packed` is exactly
    //     `pw * ph` on both of its branches, so that minimum IS `pw * ph`;
    //   * `qm::quantize_fp_qm` / `quantize_b_qm` open with their OWN
    //     `dqcoeff[..coeffs.len()].fill(0)`, so on the QM path the caller's
    //     fill was a SECOND memset of the same bytes.
    // `optimize_b` then only revises positions the quantizer already wrote.
    // CONTROL: poisoning this buffer with 0x5A5A5A5A instead of zeroing it,
    // unconditionally in a release build, leaves regression_spotcheck 102/102
    // and identity_full_8bit 1100/1100 — nothing reads an unwritten position.
    let dqcoeff: &mut [i32] = TxScratch::grown(dqcoeff_buf, pw * ph);
    let mut eob = if do_rdoq {
        let mut e = match qm {
            Some((wt, iwt)) => {
                crate::qm::quantize_fp_qm(packed, scan, qt, log_scale, wt, iwt, qcoeff, dqcoeff)
            }
            None => crate::quant::quantize_fp(packed, scan, qt, log_scale, qcoeff, dqcoeff),
        };
        if e != 0 {
            let (cut_off_num, cut_off_denum) = crate::quant::rdoq_cutoffs(frame.rdoq_level);
            let tx_class = cc::TX_TYPE_TO_CLASS[tx_type];
            let o = crate::quant::OptimizeCtx {
                txb_costs: rates.coeff.txb(cc::txsize_entropy_ctx(c_tx), plane_type),
                eob_costs: &rates.coeff.eob[cc::TXSIZE_LOG2_MINUS4[c_tx]][plane_type],
                rdmult: crate::quant::rdoq_rdmult_full(
                    frame.lambda as u32,
                    plane_type,
                    frame.sharpness,
                    false,
                    frame.sharp_tx_active && plane_type == 0,
                    frame.rdoq_allintra_rd_mult,
                ),
                sharpness_flag: frame.sharp_tx_active && plane_type == 0,
                iwt: qm.map(|(_, iwt)| iwt),
                tx_size: c_tx,
                tx_class,
                txb_skip_ctx,
                dc_sign_ctx,
                cut_off_num,
                cut_off_denum,
            };
            crate::quant::optimize_b(packed, qcoeff, dqcoeff, &mut e, scan, qt, &o);
        }
        e
    } else {
        match qm {
            Some((wt, iwt)) => {
                crate::qm::quantize_b_qm(packed, scan, qt, log_scale, wt, iwt, qcoeff, dqcoeff)
            }
            None => crate::quant::quantize_b(packed, scan, qt, log_scale, qcoeff, dqcoeff),
        }
    };
    let _ = &mut eob;
    // [SVT_HDR_MODE] fork noise normalization (see FunnelFrame field doc).
    if frame.noise_norm_strength > 0 && plane_type == 0 && eob != 0 && tx_type != 9 {
        crate::noise_norm::perform_noise_normalization(
            &qt.dequant,
            qm.map(|(_, iwt)| iwt),
            packed,
            qcoeff,
            dqcoeff,
            &mut eob,
            scan,
            c_tx,
            frame.noise_norm_strength,
        );
    }

    // ---- Reconstruction, gated on C's own predicate ----
    //
    // C: `if (ctx->mds_do_spatial_sse || (!is_inter && cand_bf->cand->
    // block_mi.tx_depth))` (product_coding_loop.c:4783-4784), with the chroma
    // twin at full_loop.c:2313 (`is_full_loop && ctx->mds_do_spatial_sse`) and
    // the single-TX luma site at :5727. The all-intra derivation pins
    // `spatial_sse_full_loop_level = 3` (SSSE_MDS3, enc_mode_config.c:10010),
    // so `mds_do_spatial_sse` is false at MDS1 (:7025) and MDS2 (:7047) and C
    // inverse-transforms NOTHING at those stages.
    //
    // `need_recon` is the caller's answer to "do I read `out.recon`", proven
    // per site at the three call sites that pass `false`. `|| spatial_dist` is
    // belt-and-braces, not redundancy: the spatial-SSE arm below reads `recon`,
    // so a future caller that mislabels `need_recon` still gets correct
    // distortion instead of an empty slice.
    let do_recon = need_recon || spatial_dist;
    // Empty, not zeroed, when skipped: a consumer that appears later gets an
    // index panic rather than a plausible black block.
    //
    // C's `ctx->recon_ptr[txt_itr]` slot (see [`TxOutBufs`]): grown, never
    // re-zeroed — every path below writes all `n` positions. `&mut []` when
    // skipped keeps the old `Vec::new()` contract (a consumer that reads it
    // gets an index panic, never a plausible black block), and `recon_len == 0`
    // carries that out to the caller.
    let recon: &mut [u8] = if do_recon {
        grown_out(&mut out.recon, n)
    } else {
        &mut []
    };
    if !do_recon {
        // C's branch is simply not taken here.
    } else if eob > 0 {
        // `dq_full` re-lays the pw x ph quantised corner into a zero-padded
        // w x h buffer at stride `w`, because the inverse dispatch reads its
        // input at the SAME stride it writes its output at. When there is no
        // 64-dim fold (`pw == w && ph == h`, i.e. every TX up to 32x32) that
        // buffer is a byte-for-byte COPY of `dqcoeff` at the same stride, so
        // the zero-fill and the row copy are both dead: pass `dqcoeff`
        // straight through. Only the 64-dim shapes, where `dqcoeff` is packed
        // at stride `pw = 32` and the dispatch wants stride `w`, still need
        // the re-lay — and there the zero-fill IS load-bearing, since only the
        // pw x ph corner is written.
        let dq_src: &[i32] = if pw == w && ph == h {
            &dqcoeff[..n]
        } else {
            let dq_full = TxScratch::zeroed(dq_full, n);
            for r in 0..ph {
                dq_full[r * w..r * w + pw].copy_from_slice(&dqcoeff[r * pw..(r + 1) * pw]);
            }
            &dq_full[..n]
        };
        if lossless_wht {
            // C `svt_aom_inv_transform_recon8bit` (inv_transforms.c:3141):
            // widens the 8-bit prediction into a u16 scratch, runs the highbd
            // inverse with bd 8 (`svt_av1_inv_txfm_add_c`, :3266), and narrows.
            // Its read and write buffers differ, so it forces `eob =
            // av1_get_max_eob(TX_4X4)` and `highbd_iwht4x4_add` (:2874) always
            // takes the 16-coefficient kernel — never the eob <= 1 shortcut.
            let mut pred16 = [0u16; 16];
            for r in 0..4 {
                for c in 0..4 {
                    pred16[r * 4 + c] = u16::from(pred[pred_off + r * pred_stride + c]);
                }
            }
            let mut out16 = [0u16; 16];
            svtav1_dsp::inv_txfm::highbd_iwht4x4_16_add(dq_src, &pred16, 4, &mut out16, 4, 8);
            for (d, &s) in recon.iter_mut().zip(out16.iter()) {
                *d = s as u8;
            }
        } else {
            let inv = TxScratch::zeroed(inv, n);
            let ok = svtav1_dsp::txfm_dispatch::inv_txfm2d_dispatch(
                dq_src,
                inv,
                w,
                TX_SIZE_FROM_C[c_tx],
                rs_tx_type,
            );
            debug_assert!(ok, "inv txfm {w}x{h} type {tx_type}");
            svtav1_dsp::residual::recon_add_clamp(&pred[pred_off..], pred_stride, inv, w, h, recon);
        }
    } else {
        for r in 0..h {
            let prow = pred_off + r * pred_stride;
            recon[r * w..(r + 1) * w].copy_from_slice(&pred[prow..prow + w]);
        }
    }

    let dist = if spatial_dist {
        // C `svt_spatial_full_distortion_kernel_facade(..., cropped_tx_width,
        // cropped_tx_height, ...)`: the SSE walks the CROPPED extent (the recon
        // keeps its full `w` stride), so a straddling boundary TX block is
        // priced only over its in-frame part.
        let mut sse: u64 =
            svtav1_dsp::variance::sse(&src[src_off..], src_stride, recon, w, crop_w, crop_h);
        // [SVT_HDR_MODE] fork tx-bias facade layer (pic_operators.c:252):
        // the spatial SSE is biased by prediction-mode class + tx size
        // BEFORE the psy add (the facade IS the SSE producer at the C call
        // sites; get_svt_psy_full_dist is added by the caller after). The
        // luma and chroma mode-class index sets are identical (DC/SMOOTH*
        // blurry, V/H/PAETH neutral), so one mapping serves both planes.
        // Stills are temporal layer 0, and the facade's ac_bias param only
        // feeds an `== 0.0` gate, so the effective flag is equivalent.
        if frame.tx_bias > 0 {
            let class = match intra_dir {
                0 | 9 | 10 | 11 => crate::tx_bias::BiasModeClass::IntraBlurry,
                1 | 2 | 12 => crate::tx_bias::BiasModeClass::IntraNeutral,
                _ => crate::tx_bias::BiasModeClass::IntraOther,
            };
            sse = crate::tx_bias::facade_bias(
                sse as i64,
                class,
                true,
                // The facade IS the cropped-dims call site in C
                // (`svt_spatial_full_distortion_kernel_facade(...,
                // cropped_tx_width, cropped_tx_height, ...)`), so its
                // area/size inputs are the CROPPED ones too.
                crop_w as u32,
                crop_h as u32,
                0,
                if frame.ac_bias_eff > 0.0 { 1.0 } else { 0.0 },
                frame.tx_bias,
            ) as u64;
        }
        // [ac-bias] C adds llrint(psy_distortion * effective_ac_bias) to
        // the spatial SSE BEFORE the <<4 (get_svt_psy_full_dist call sites
        // in full_loop.c). tx_bias=0 (fork default) keeps the facade a
        // plain SSE, so this is the whole fork-default delta here.
        if frame.ac_bias_eff > 0.0 {
            // C `get_svt_psy_full_dist(..., cropped_tx_width,
            // cropped_tx_height, ...)` (product_coding_loop.c:4834/:4862,
            // :5803/:5831) — cropped area, full recon stride.
            sse += svtav1_dsp::ac_bias::psy_full_dist(
                src,
                src_off,
                src_stride,
                recon,
                0,
                w,
                crop_w,
                crop_h,
                frame.ac_bias_eff,
            );
        }
        sse << 4
    } else {
        // Freq-domain: svt_aom_picture_full_distortion32_bits_single
        // (RESIDUAL) + three_quad + RIGHT_SIGNED_SHIFT((1 - scale) * 2).
        let mut d: u64 = if eob > 0 {
            svtav1_dsp::residual::sse_i32(&packed[..pw * ph], &dqcoeff[..pw * ph])
        } else {
            svtav1_dsp::residual::sq_sum_i32(&packed[..pw * ph])
        };
        d += three_quad_energy;
        let shift = (1 - log_scale) * 2;
        if shift < 0 { d << (-shift) } else { d >> shift }
    };

    // ---- coefficient rate: C's `if / else if / else` tier, in C's order ----
    //
    // C never calls the real estimator when a closed form applies — the tiers
    // are literally an if/else-if/else and only the taken arm runs
    // (product_coding_loop.c:4914-4934, identically :5540-5564, :5883-5890).
    // The port used to compute `cost_coeffs_txb` FIRST and then throw it away
    // on the closed-form branches. Same numbers, in C's order.
    //
    // `coeff_rate_est_lvl == 2` (M7/M8 allintra, rate_est_level 4): the LUMA
    // coeff RATE used in the RD compare is the fast per-txb approximation, not
    // the real entropy cost — `th = (txw*txh)>>6`, `eob < th ? 6000+eob*1000 :
    // real`. C applies it identically in every luma tx path (tx_type_search
    // product_coding_loop.c:4976, perform_dct_dct_tx :5619, the multi-txb loop
    // :5951), all reached from the shared `full_loop_core`, so it prices BOTH
    // the MDS1 NIC pruning and the MDS3 mode/tx-type decision. Chroma keeps the
    // real cost here; its own eob-approximation (`skip_chroma_rate_est`,
    // full_loop.c:1922) is applied by the caller. Level 1 (M6) keeps the real
    // cost. `eob==0` folds into `eob < th` (th >= 1 for every >= 8x8 TX) ->
    // 6000, matching C's tx_type_search / coeff-shaving eob==0 luma price.
    //
    // `RateMode::Lvl0Closed` is level 0 (eff-M9) on the
    // `perform_tx_partitioning` path: the depth loop applied this same formula
    // to `tx_unit`'s output and dropped the exact rate entirely. Producing it
    // here is the identical arithmetic on the identical inputs (`eob`, `w*h`),
    // so the depth loop's own expression — left untouched — still computes the
    // same number it always did.
    let closed_lvl2 =
        plane_type == 0 && frame.cfg.coeff_rate_est_lvl == 2 && (eob as usize) < ((w * h) >> 6);
    let bits = match rate_mode {
        RateMode::Lvl0Closed => {
            let th = (w * h) >> 6;
            if (eob as usize) < th {
                6000 + eob as i32 * 1000
            } else {
                3000 + eob as i32 * 100
            }
        }
        RateMode::Exact if closed_lvl2 => 6000 + eob as i32 * 1000,
        RateMode::Exact if eob > 0 => cost_coeffs_txb(
            qcoeff,
            eob,
            c_tx,
            tx_type,
            plane_type,
            txb_skip_ctx,
            dc_sign_ctx,
            intra_dir,
            rates,
        ),
        RateMode::Exact => cost_skip_txb(c_tx, plane_type, txb_skip_ctx, rates),
    };
    let cul = compute_cul_level(scan, qcoeff, eob);

    Some(TxUnitMeta {
        eob,
        q_len: pw * ph,
        recon_len: recon.len(),
        dist,
        bits,
        cul,
    })
}

pub(super) fn energy_region(coeffs: &[i32], stride: usize, w: usize, h: usize) -> u64 {
    let mut e: u64 = 0;
    for r in 0..h {
        for c in 0..w {
            let v = coeffs[r * stride + c] as i64;
            e += (v * v) as u64;
        }
    }
    e
}

// ===========================================================================
// bd10 u16 MD path (task #94): high-bit-depth mirrors of the intra-block
// chain. ADDITIVE — the u8 predict_unit / tx_unit above are untouched, so the
// bd8 path is byte-identical. These run only from the bd10 re-encode pass
// (pipeline.rs), gated on bit_depth == 10.
// ===========================================================================

/// u16 mirror of [`predict_unit`] for the bd10 MD path. Uses the C-verified
/// hbd predictor kernels (`svtav1_dsp::hbd`) and [`crate::partition::
/// extract_neighbors_hbd`]. Directional / filter-intra modes are not yet
/// ported here (the first bd10 cell — gradient 64x64 preset13 — resolves to
/// DC-only leaves); they panic LOUDLY rather than predict wrong pixels, so a
/// future non-DC bd10 cell is an obvious follow-up, never a silent corruption.
#[allow(clippy::too_many_arguments)]
pub(crate) fn predict_unit_hbd(
    recon: &[u16],
    stride: usize,
    abs_x: usize,
    abs_y: usize,
    w: usize,
    h: usize,
    mode: u8,
    delta: i8,
    fi_mode: u8,
    geom: &UnitGeom,
    edge_filter: bool,
    filt_type: i32,
    dst: &mut [u16],
    bd: u8,
) {
    use svtav1_dsp::hbd as hp;
    // Directional: modes D45..D203 (3..=8) OR V/H with a nonzero angle delta.
    // Mirrors the u8 `predict_unit` directional arm: same DrGeom, routed to the
    // hbd edge/kernel twin `dr_predict_hbd`. (task #94 follow-up)
    if matches!(mode, 3..=8) || (matches!(mode, 1 | 2) && delta != 0) {
        let p_angle = crate::intra_edge::MODE_TO_ANGLE_MAP[mode as usize] + delta as i32 * 3;
        debug_assert!(fi_mode == FI_NONE);
        let g = crate::intra_edge::DrGeom {
            px: abs_x,
            py: abs_y,
            txw: w,
            txh: h,
            mi_row: geom.mi_row,
            mi_col: geom.mi_col,
            bw_px: geom.bw_px,
            bh_px: geom.bh_px,
            row_off: 0,
            col_off: 0,
            ss: geom.ss,
            frame_w: geom.frame_w,
            frame_h: geom.frame_h,
            sb_mi_size: geom.sb_mi_size,
            tile: geom.tile,
        };
        crate::intra_edge::dr_predict_hbd(
            |x, y| recon[y * stride + x],
            &g,
            p_angle,
            edge_filter,
            filt_type,
            svtav1_types::partition::PartitionType::None,
            dst,
            bd,
        );
        return;
    }
    let (above, left, top_left, has_above, has_left) = crate::partition::extract_neighbors_hbd(
        recon,
        stride,
        abs_x,
        abs_y,
        w,
        h,
        bd,
        // ISSUE #18: tile-scoped neighbour availability, exactly as the u8
        // twin `predict_unit` passes it. The directional arm above already
        // carried `geom.tile` into `dr_predict_hbd`; this arm (DC / V / H /
        // smooth* / paeth / filter-intra) did not, so a forced multi-tile
        // bd10 frame predicted across the tile edge. `geom.tile` is the whole
        // frame for a single-tile encode, where both are 0 and this is
        // bit-for-bit the previous behaviour.
        geom.tile.top_px(geom.ss),
        geom.tile.left_px(geom.ss),
        // C n_top_px/n_left_px: this plane's ALIGNED extent (u8 twin's comment).
        geom.frame_w >> geom.ss,
        geom.frame_h >> geom.ss,
    );
    if fi_mode != FI_NONE {
        // Filter-intra (highbd). C `build_intra_predictors_high` sets
        // above_row[-1] via the standard need_above_left logic (the base=512
        // fallback for the frame corner) — which is exactly `top_left` from
        // extract_neighbors_hbd — then calls
        // `svt_aom_highbd_filter_intra_predictor(above_row, left_col, ...)`.
        // `predict_filter_intra_hbd` expects `above[0]` = top-left,
        // `above[1..]` = the above row. Mirrors the u8 `predict_unit` fi arm.
        let mut above_c = alloc::vec![0u16; w + 1];
        above_c[0] = top_left;
        above_c[1..].copy_from_slice(&above);
        hp::predict_filter_intra_hbd(dst, w, &above_c, &left, w, h, fi_mode, bd);
        return;
    }
    match mode {
        0 => hp::predict_dc_hbd(dst, w, &above, &left, w, h, has_above, has_left, bd),
        1 => hp::predict_v_hbd(dst, w, &above, w, h),
        2 => hp::predict_h_hbd(dst, w, &left, w, h),
        9 => hp::predict_smooth_hbd(dst, w, &above, &left, w, h),
        10 => hp::predict_smooth_v_hbd(dst, w, &above, &left, w, h),
        11 => hp::predict_smooth_h_hbd(dst, w, &above, &left, w, h),
        12 => hp::predict_paeth_hbd(dst, w, &above, &left, top_left, w, h),
        m => unreachable!("funnel bd10 mode {m}"),
    }
}

/// u16 mirror of [`TxUnitOut`] — recon in the 10-bit domain.
pub(crate) struct TxUnitOutHbd {
    pub eob: u16,
    /// Packed (32-capped) quantized levels — the CODED levels.
    pub qcoeff: Vec<i32>,
    /// Reconstructed pixels (w x h raster, 10-bit).
    pub recon: Vec<u16>,
    /// `(dc_sign << 6) | min(cul_level, 63)` neighbor byte.
    pub cul: u8,
    /// RD distortion in the 10-bit domain — the freq-domain RESIDUAL form
    /// (MDS1) or spatial SSE << 4 (MDS3), matching [`TxUnitOut::dist`].
    /// ZERO unless the caller passed [`TxRdArgs`] (the level-only re-encode
    /// post-pass does not, so it stays byte-inert).
    pub dist: u64,
    /// Coefficient bits (or skip-txb bits when `eob == 0`), matching
    /// [`TxUnitOut::bits`]. ZERO unless [`TxRdArgs`] was passed.
    pub bits: i32,
}

/// Opt-in RD outputs for [`tx_unit_hbd`].
///
/// `tx_unit_hbd` began life as a LEVEL producer for the bd10 re-encode
/// post-pass, which needs no RD terms. The bd10 full-RD stages (MDS1/MDS3)
/// need exactly the two the u8 [`tx_unit`] returns, in the same domains and
/// with the same shifts — so they are computed here, but only when asked.
/// `None` keeps every existing caller bit-for-bit unchanged.
pub(crate) struct TxRdArgs {
    /// MDS3 (recon-vs-source spatial SSE << 4) when true; the MDS1
    /// freq-domain residual form when false. Mirrors `tx_unit`'s flag.
    pub spatial_dist: bool,
    /// Intra direction feeding the ext-tx-type rate row (fi-MAPPED for
    /// FILTER candidates, exactly as the u8 sites do it).
    pub intra_dir: usize,
    /// `FunnelCfg::coeff_rate_est_lvl` — level 2 (M7/M8) replaces the LUMA
    /// coeff rate with C's fast per-txb approximation.
    pub coeff_rate_est_lvl: u8,
    /// `[SVT_HDR_MODE]` fork tx-bias facade strength (`FunnelFrame::tx_bias`).
    /// The facade is pure arithmetic on the SSE, so it applies at any depth.
    pub tx_bias: u8,
    /// The cropped-TX distortion extent (`frame_geom::cropped_tx_dims` /
    /// `_uv`), exactly as the u8 [`tx_unit`]'s `crop` — C reaches the same
    /// `svt_spatial_full_distortion_kernel_facade` at both depths (only the
    /// kernel behind it is bit-depth-selected), so the cropped dims are the
    /// same inputs. Clips ONLY the spatial arm; `(w, h)` on a 64-aligned
    /// frame.
    pub crop: (usize, usize),
}

/// bd10 FULL-RD context for the MDS1 / MDS3 stages (task #94, MODE axis).
///
/// At `hbd_md != 0` — i.e. every M0..M13 bd10 frame (DUAL, see
/// docs/bd10-port-map.md) — C runs the whole full-RD chain at TRUE 10 bits:
/// the prediction, the residual, the quantizer table, the lambda and the
/// distortion kernel are all the 10-bit ones. Below eff-M9 `nic_counts` is
/// (6,6,6) or wider, so the coded mode is NOT the MDS0 survivor — it is the
/// MDS1/MDS3 full-RD winner, and deciding it on 8-bit pixels picks C's *bd8*
/// winner. That is the entire p6 MODE-flip class.
///
/// This carries the bit-depth-specific inputs those stages need. It is `Some`
/// only when [`FunnelCtx::full_rd10`] is set (bd10, complete-SB, mainline
/// tools); the u8 path never constructs one and is byte-identical.
pub(super) struct Bd10Rd {
    /// The block's TRUE 10-bit luma source, w*h at stride w. The harness
    /// ingestion model is `src10 == src8 << (bd - 8)` (docs/bd10-port-map.md
    /// "PORT: use plain u16 planes"), the same relation `hadamard_satd_hbd`
    /// and the re-encode post-pass already assume.
    pub(super) y_src10: Vec<u16>,
    /// The block's 10-bit chroma sources, cw*chh at stride cw. Empty when the
    /// block carries no chroma (`has_uv == 0`).
    pub(super) u_src10: Vec<u16>,
    pub(super) v_src10: Vec<u16>,
    /// bd10 quant tables (`build_quant_table_bd`): Q10 is ~4x Q8 but NOT
    /// exactly, which is precisely why the RD ordering is not scale-invariant.
    pub(super) qt: QuantTable,
    pub(super) qt_u: QuantTable,
    pub(super) qt_v: QuantTable,
    /// C `full_lambda_md[EB_10_BIT_MD]` (md_process.c:753) — the bd10 rdmult
    /// base, x16. NOT a x16 of the bd8 lambda; see `kf_full_lambda_bd10`.
    pub(super) lambda: u64,
    pub(super) bd: u8,
}

/// u16 / bd10 mirror of the level-producing core of [`tx_unit`]: 10-bit
/// residual -> forward TX -> Q10 quantize (+ optional RDOQ) -> 10-bit recon.
///
/// The forward/inverse transforms are bit-depth-INDEPENDENT (i32 coeffs) and
/// the quantize/RDOQ kernels are table-driven, so this reuses them verbatim;
/// only the residual (u16 src/pred), the quant table (`qt` = the bd10 row),
/// and the recon-add clip (`clip_pixel_highbd(bd)`) are bit-depth-specific.
/// ac-bias / noise-norm are NOT applied here (both are fork-only and both need
/// a u16 psy kernel that is not ported; the bd10 full-RD funnel refuses to
/// engage when either is active, so this is never silently wrong). RD
/// distortion + coeff bits are computed only when `rd` is `Some` — the
/// level-only re-encode post-pass passes `None` and is byte-inert.
/// `txb_skip_ctx` / `dc_sign_ctx` are the RDOQ contexts (0/0 at eff-M9,
/// rate_est_level 0) and, when `rd` is set, also the coeff-rate contexts.
#[allow(clippy::too_many_arguments)]
pub(crate) fn tx_unit_hbd(
    src: &[u16],
    src_stride: usize,
    src_off: usize,
    pred: &[u16],
    pred_stride: usize,
    pred_off: usize,
    w: usize,
    h: usize,
    tx_type: usize,
    plane_type: usize,
    txb_skip_ctx: usize,
    dc_sign_ctx: usize,
    qt: &QuantTable,
    rdoq_level: u8,
    lambda: u64,
    sharpness: i8,
    allintra_rd_mult: bool,
    rates: &MdRates,
    do_rdoq: bool,
    bd: u8,
    qm_level: u8,
    rd: Option<&TxRdArgs>,
) -> TxUnitOutHbd {
    tx_unit_hbd_screened(
        src,
        src_stride,
        src_off,
        pred,
        pred_stride,
        pred_off,
        w,
        h,
        tx_type,
        plane_type,
        txb_skip_ctx,
        dc_sign_ctx,
        qt,
        rdoq_level,
        lambda,
        sharpness,
        allintra_rd_mult,
        rates,
        do_rdoq,
        bd,
        qm_level,
        rd,
        None,
    )
    .expect("no screen was passed, so the unit is always committed")
}

/// [`tx_unit_hbd`] with C's SATD early exit wired into its middle — the bd10
/// twin of [`tx_unit_screened`], and the arm `txt_search` uses when the 10-bit
/// context is present.
///
/// C computes its trial SATD on `ctx->tx_coeffs` at the ACTIVE MD bit depth
/// (`svt_aom_estimate_transform`'s `ctx->hbd_md ? EB_TEN_BIT : EB_EIGHT_BIT`,
/// `product_coding_loop.c:4737`), so when the port runs bd10 full-RD the gate
/// belongs on THIS transform, not on the u8 twin's — which is why `txt_search`
/// screens here first and only computes the u8 unit for admitted trials.
#[allow(clippy::too_many_arguments)]
pub(super) fn tx_unit_hbd_screened(
    src: &[u16],
    src_stride: usize,
    src_off: usize,
    pred: &[u16],
    pred_stride: usize,
    pred_off: usize,
    w: usize,
    h: usize,
    tx_type: usize,
    plane_type: usize,
    txb_skip_ctx: usize,
    dc_sign_ctx: usize,
    qt: &QuantTable,
    rdoq_level: u8,
    lambda: u64,
    sharpness: i8,
    // C `scs->allintra || scs->static_config.rtc` — the RDOQ plane rate
    // weight arm (`crate::quant::PLANE_RD_MULT`). FALSE on a video frame.
    allintra_rd_mult: bool,
    rates: &MdRates,
    do_rdoq: bool,
    bd: u8,
    qm_level: u8,
    rd: Option<&TxRdArgs>,
    screen: Option<&mut SatdScreen>,
) -> Option<TxUnitOutHbd> {
    let n = w * h;
    let c_tx = cc::tx_size_from_dims(w, h);
    let rs_tx_type = TX_TYPE_FROM_C[tx_type];

    // Build directly (uninit capacity + push) rather than `vec![0; n]` + full
    // overwrite: every element is written below, so the zero-fill was dead. This
    // pushes exactly h*w = n values in row-major order — byte-identical contents,
    // no `calloc`/`memset`.
    let mut residual = Vec::with_capacity(n);
    for r in 0..h {
        let srow = src_off + r * src_stride;
        let prow = pred_off + r * pred_stride;
        for c in 0..w {
            residual.push(src[srow + c] as i32 - pred[prow + c] as i32);
        }
    }
    let mut coeffs = vec![0i32; n];
    let ok = svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(
        &residual,
        &mut coeffs,
        w,
        rs_tx_size(w, h),
        rs_tx_type,
    );
    debug_assert!(ok, "bd10 fwd txfm {w}x{h} type {tx_type}");

    // C's SATD early exit, at C's position — see [`tx_unit_screened`]'s
    // comment at the same point in the u8 pipeline.
    if let Some(screen) = screen
        && !screen.admit(&coeffs[..n])
    {
        return None;
    }

    // 64-dim fold (svt_handle_transform64x64): keep the 32-capped low-freq
    // quadrant packed at the adjusted stride, exactly like tx_unit. The energy
    // of the DISCARDED region is only needed by the freq-domain distortion, so
    // it is gathered only when RD terms were asked for (byte-inert otherwise).
    let (pw, ph) = (w.min(32), h.min(32));
    let mut three_quad_energy: u64 = 0;
    let packed = if w > 32 || h > 32 {
        if rd.is_some() {
            // Identical region geometry to `tx_unit` (svt_handle_transform64x64
            // / 64x32 / 32x64, transforms.c:3223) — the transforms are
            // bit-depth-independent so the same three quadrants are dropped.
            if w == 64 && h == 64 {
                three_quad_energy = energy_region(&coeffs[32..], 64, 32, 32)
                    + energy_region(&coeffs[32 * 64..], 64, 64, 32);
            } else if w == 64 {
                three_quad_energy = energy_region(&coeffs[32..], 64, 32, h.min(32));
            } else {
                three_quad_energy = energy_region(&coeffs[32 * w..], w, w, h - 32);
            }
        }
        // Uninit capacity + extend rather than `vec![0; pw*ph]` + full copy: the
        // loop copies every one of the pw*ph elements, so the zero-fill was dead.
        // Byte-identical contents (same pw-wide rows in order), no `calloc`/`memset`.
        let mut v = Vec::with_capacity(pw * ph);
        for r in 0..ph {
            v.extend_from_slice(&coeffs[r * w..r * w + pw]);
        }
        v
    } else {
        coeffs.clone()
    };

    let scan = crate::entropy::scan_tables::scan(
        c_tx,
        crate::entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[tx_type] as usize,
    );
    let log_scale = TX_SCALE_TAB[c_tx];
    let mut qcoeff = vec![0i32; pw * ph];
    let mut dqcoeff = vec![0i32; pw * ph];
    // [SVT_HDR_MODE] QM at bd10. C selects the matrix by qm_level regardless of
    // bit depth (svt_av1_qm_init, md_config_process.c:246-280 — a pure function
    // of base_qindex) and then routes bd>8 through the *_qm HIGHBD kernels via
    // svt_av1_highbd_quantize_{b,fp}_facade (full_loop.c:139-176). This path
    // previously always called the NON-QM highbd kernels and passed `iwt: None`
    // to the trellis, so fork mode (QM on by default) silently dequantized
    // without matrices at bd10 while bd8 applied them. Same 2D-only gate as the
    // bd8 site: the caller passes qm_level 15 for non-2D tx types.
    // IS_2D_TRANSFORM(tx_type) == tx_type < IDTX(9) — definitions.h:1122, the
    // same gate the bd8 sites use (tx_unit:1438, encode_loop.rs:213).
    let qm = if tx_type < 9 && qm_level < 15 {
        crate::qm::qm_slices(usize::from(qm_level), plane_type == 1, c_tx)
    } else {
        None
    };
    let eob = if do_rdoq {
        let mut e = match qm {
            Some((wt, iwt)) => crate::qm::quantize_fp_hbd_qm(
                &packed,
                scan,
                qt,
                log_scale,
                wt,
                iwt,
                &mut qcoeff,
                &mut dqcoeff,
            ),
            None => crate::quant::quantize_fp_hbd(
                &packed,
                scan,
                qt,
                log_scale,
                &mut qcoeff,
                &mut dqcoeff,
            ),
        };
        if e != 0 {
            let (cut_off_num, cut_off_denum) = crate::quant::rdoq_cutoffs(rdoq_level);
            let tx_class = cc::TX_TYPE_TO_CLASS[tx_type];
            let o = crate::quant::OptimizeCtx {
                txb_costs: rates.coeff.txb(cc::txsize_entropy_ctx(c_tx), plane_type),
                eob_costs: &rates.coeff.eob[cc::TXSIZE_LOG2_MINUS4[c_tx]][plane_type],
                rdmult: crate::quant::rdoq_rdmult_full(
                    lambda as u32,
                    plane_type,
                    sharpness,
                    false,
                    false,
                    allintra_rd_mult,
                ),
                sharpness_flag: false,
                // The trellis dequant must use the SAME matrix as the quantize
                // (C optimize_b reads qparam->iqmatrix through get_dqv).
                iwt: qm.map(|(_, iwt)| iwt),
                tx_size: c_tx,
                tx_class,
                txb_skip_ctx,
                dc_sign_ctx,
                cut_off_num,
                cut_off_denum,
            };
            crate::quant::optimize_b(&packed, &mut qcoeff, &mut dqcoeff, &mut e, scan, qt, &o);
        }
        e
    } else {
        // rdoq level 0 (do_rdoq == false): C routes bd>8 to the highbd b-quant
        // (no INT16 clamp) — the SAME clamp-is-bd8-only class as the fp fix.
        match qm {
            Some((wt, iwt)) => crate::qm::quantize_b_hbd_qm(
                &packed,
                scan,
                qt,
                log_scale,
                wt,
                iwt,
                &mut qcoeff,
                &mut dqcoeff,
            ),
            None => crate::quant::quantize_b_hbd(
                &packed,
                scan,
                qt,
                log_scale,
                &mut qcoeff,
                &mut dqcoeff,
            ),
        }
    };

    // 10-bit reconstruction (pred + inverse residual, clipped to [0, 2^bd-1]).
    let mut recon = vec![0u16; n];
    if eob > 0 {
        let mut dq_full = vec![0i32; n];
        for r in 0..ph {
            dq_full[r * w..r * w + pw].copy_from_slice(&dqcoeff[r * pw..(r + 1) * pw]);
        }
        let mut inv = vec![0i32; n];
        let ok = svtav1_dsp::txfm_dispatch::inv_txfm2d_dispatch_bd(
            &dq_full,
            &mut inv,
            w,
            rs_tx_size(w, h),
            rs_tx_type,
            bd,
        );
        debug_assert!(ok, "bd10 inv txfm {w}x{h} type {tx_type}");
        let maxv = (1i32 << bd) - 1;
        for r in 0..h {
            let prow = pred_off + r * pred_stride;
            for c in 0..w {
                recon[r * w + c] = (pred[prow + c] as i32 + inv[r * w + c]).clamp(0, maxv) as u16;
            }
        }
    } else {
        for r in 0..h {
            let prow = pred_off + r * pred_stride;
            for c in 0..w {
                recon[r * w + c] = pred[prow + c];
            }
        }
    }

    // RD terms (MDS1/MDS3 only) — the same two domains and shifts as the u8
    // `tx_unit`, on 10-bit inputs. C reaches them through the SAME facades at
    // both depths; only the kernel behind the facade is bit-depth-selected:
    //   spatial: svt_spatial_full_distortion_kernel_facade
    //            (pic_operators.c:257) dispatches `hbd_md ?
    //            svt_full_distortion_kernel16_bits : svt_spatial_full_
    //            distortion_kernel` -> a plain u16 SSE at bd10, then the
    //            caller's `<<= 4` (product_coding_loop.c:5836-5837).
    //   freq:    svt_aom_picture_full_distortion32_bits_single (pic_operators.c
    //            :172) is bit-depth-INDEPENDENT (i32 coefficients), so the u8
    //            expression is reused verbatim on the bd10 coefficients.
    // The coefficient RATE tables are qindex-driven with no bit-depth term, so
    // `rates` is shared with the u8 path unchanged.
    let (dist, bits) = match rd {
        None => (0u64, 0i32),
        Some(a) => {
            let dist = if a.spatial_dist {
                // Cropped area, FULL recon stride — the bd10 twin of the u8
                // site (C passes `cropped_tx_width`/`_height` into the same
                // facade at both depths).
                let (crop_w, crop_h) = a.crop;
                debug_assert!(crop_w <= w && crop_h <= h, "crop must clip, never extend");
                let mut sse = svtav1_dsp::hbd::full_distortion_kernel16_bits(
                    src, src_off, src_stride, &recon, 0, w, crop_w, crop_h,
                );
                // [SVT_HDR_MODE] fork tx-bias facade (pic_operators.c:265-292):
                // pure integer scaling of the SSE by prediction-mode class, so
                // it is bit-depth-agnostic and mirrors the u8 site exactly.
                if a.tx_bias > 0 {
                    let class = match a.intra_dir {
                        0 | 9 | 10 | 11 => crate::tx_bias::BiasModeClass::IntraBlurry,
                        1 | 2 | 12 => crate::tx_bias::BiasModeClass::IntraNeutral,
                        _ => crate::tx_bias::BiasModeClass::IntraOther,
                    };
                    sse = crate::tx_bias::facade_bias(
                        sse as i64,
                        class,
                        true,
                        crop_w as u32,
                        crop_h as u32,
                        0,
                        0.0,
                        a.tx_bias,
                    ) as u64;
                }
                sse << 4
            } else {
                let mut d: u64 = 0;
                if eob > 0 {
                    for i in 0..pw * ph {
                        let e = (packed[i] - dqcoeff[i]) as i64;
                        d += (e * e) as u64;
                    }
                } else {
                    for i in 0..pw * ph {
                        d += (packed[i] as i64 * packed[i] as i64) as u64;
                    }
                }
                d += three_quad_energy;
                let shift = (1 - log_scale) * 2;
                if shift < 0 { d << (-shift) } else { d >> shift }
            };
            let real_bits = if eob > 0 {
                cost_coeffs_txb(
                    &qcoeff,
                    eob,
                    c_tx,
                    tx_type,
                    plane_type,
                    txb_skip_ctx,
                    dc_sign_ctx,
                    a.intra_dir,
                    rates,
                )
            } else {
                cost_skip_txb(c_tx, plane_type, txb_skip_ctx, rates)
            };
            // C `coeff_rate_est_lvl == 2` LUMA fast approximation — identical
            // to the u8 site (see its comment); bit-depth-independent.
            let bits = if plane_type == 0 && a.coeff_rate_est_lvl == 2 {
                let th = (w * h) >> 6;
                if (eob as usize) < th {
                    6000 + eob as i32 * 1000
                } else {
                    real_bits
                }
            } else {
                real_bits
            };
            (dist, bits)
        }
    };

    let cul = compute_cul_level(scan, &qcoeff, eob);
    Some(TxUnitOutHbd {
        eob,
        qcoeff,
        recon,
        cul,
        dist,
        bits,
    })
}

use crate::quant::TX_SCALE_TAB;

/// C TxType index -> Rust TxType (identical numbering).
pub(super) const TX_TYPE_FROM_C: [svtav1_types::transform::TxType; 16] = {
    use svtav1_types::transform::TxType::*;
    [
        DctDct,
        AdstDct,
        DctAdst,
        AdstAdst,
        FlipAdstDct,
        DctFlipAdst,
        FlipAdstFlipAdst,
        AdstFlipAdst,
        FlipAdstAdst,
        Idtx,
        VDct,
        HDct,
        VAdst,
        HAdst,
        VFlipAdst,
        HFlipAdst,
    ]
};

/// The port's `TxSize` for each C `TxSize` INDEX (`coeff_c::TX_*`, C's
/// `TX_SIZES_ALL` order) — the two enumerations of the same 19 shapes.
///
/// Indexed, not matched on `(w, h)`: `tx_unit_inner` already computes the C
/// index once as `c_tx`, and the second 19-arm branch chain was 6,984,051 calls
/// and 167.4 M Ir (0.36 % of a 512x512 photo frame at preset 2) for a value it
/// was holding. Pinned against `rs_tx_size_match` by
/// `rs_tx_size_table_matches_the_match_form`.
#[rustfmt::skip]
pub(super) const TX_SIZE_FROM_C: [svtav1_types::transform::TxSize; 19] = {
    use svtav1_types::transform::TxSize::*;
    [
        Tx4x4, Tx8x8, Tx16x16, Tx32x32, Tx64x64,
        Tx4x8, Tx8x4, Tx8x16, Tx16x8, Tx16x32,
        Tx32x16, Tx32x64, Tx64x32, Tx4x16, Tx16x4,
        Tx8x32, Tx32x8, Tx16x64, Tx64x16,
    ]
};

/// `(w, h)` -> the port's `TxSize`, for the call sites that do not already hold
/// the C index. Prefer `TX_SIZE_FROM_C[c_tx]` where `c_tx` is in scope.
#[inline]
pub(super) fn rs_tx_size(w: usize, h: usize) -> svtav1_types::transform::TxSize {
    TX_SIZE_FROM_C[cc::tx_size_from_dims(w, h)]
}

/// The readable form of [`rs_tx_size`], kept as the table's oracle.
#[cfg(test)]
pub(super) fn rs_tx_size_match(w: usize, h: usize) -> Option<svtav1_types::transform::TxSize> {
    use svtav1_types::transform::TxSize;
    Some(match (w, h) {
        (4, 4) => TxSize::Tx4x4,
        (8, 8) => TxSize::Tx8x8,
        (16, 16) => TxSize::Tx16x16,
        (32, 32) => TxSize::Tx32x32,
        (64, 64) => TxSize::Tx64x64,
        (4, 8) => TxSize::Tx4x8,
        (8, 4) => TxSize::Tx8x4,
        (8, 16) => TxSize::Tx8x16,
        (16, 8) => TxSize::Tx16x8,
        (16, 32) => TxSize::Tx16x32,
        (32, 16) => TxSize::Tx32x16,
        (32, 64) => TxSize::Tx32x64,
        (64, 32) => TxSize::Tx64x32,
        (4, 16) => TxSize::Tx4x16,
        (16, 4) => TxSize::Tx16x4,
        (8, 32) => TxSize::Tx8x32,
        (32, 8) => TxSize::Tx32x8,
        (16, 64) => TxSize::Tx16x64,
        (64, 16) => TxSize::Tx64x16,
        _ => return None,
    })
}
