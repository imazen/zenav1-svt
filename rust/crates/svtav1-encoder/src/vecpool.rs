//! Per-thread recycling for the transform pipeline's OUTPUT buffers.
//!
//! # Why this exists
//!
//! C allocates `ctx->quant_coeff_ptr[txt_itr]` and `ctx->recon_ptr[txt_itr]`
//! ONCE per encoder thread, in `svt_aom_mode_decision_context_ctor`
//! (`md_process.c:214`, reached from `svt_aom_enc_dec_context_ctor`'s `EB_NEW`
//! at `enc_dec_process.c:108`), and reuses them for every tx-type trial of
//! every transform unit of every block of every frame. That is why C's whole
//! encode is ~466 K allocator calls while the port's was 6.35 M: the port's
//! `tx_unit` handed each caller a freshly-allocated `Vec` per transform unit.
//!
//! Measured on the canonical alloc cell (`tools/heaptrack_alloc_cell.sh`,
//! 512x320 gb82-sc/windows.png qp20), `grown_out` at `tx_pipeline.rs:873` was
//! **1,661,174 allocating calls** — the single largest site in the encoder,
//! spread across `txt_search`, `chroma::eval_uv`, `inject_candidates`, `mds3`,
//! `md_cfl_rd_pick_alpha` and `run_mds1` rather than concentrated in one
//! caller. A per-site fix would have had to touch all six; recycling at the
//! `Vec` level fixes them at once, which is why it is done here.
//!
//! # What it does NOT change
//!
//! Nothing about the numbers. A recycled buffer is handed back to
//! [`tx_pipeline::grown_out`], which grows it to the unit's exact length and
//! whose doc carries the per-path proof that every returned position is
//! written before it is read. The pool only decides where the memory came
//! from.
//!
//! # No-std
//!
//! Recycling needs a thread-local, so it is `feature = "std"` only. Without
//! `std` a [`PoolVec`] is exactly a `Vec` and every constructor allocates,
//! which is the behaviour this module replaced.

use alloc::vec::Vec;
use core::ops::{Deref, DerefMut};

/// Number of SIZE CLASSES. Class `k` holds buffers whose capacity is in
/// `[2^k, 2^(k+1))`, so a request for `n` elements is served from class
/// `ceil(log2(n))` and every member of that class is guaranteed to be big
/// enough. Class 20 is a million elements; anything larger is not parked.
///
/// SIZE CLASSES ARE NOT OPTIONAL. A single free list mixing a 16-element
/// coefficient row with a 4096-element block recon means a small buffer is
/// handed to a large request and immediately `realloc`ed. MEASURED: with one
/// undifferentiated list, `finish_grow` (Vec growth) was 630,361 + 336,405
/// allocating calls on the canonical alloc cell -- the pool had converted
/// fresh allocations into reallocations rather than removing them.
#[cfg(feature = "std")]
const CLASS_COUNT: usize = 21;

/// How many buffers one thread parks PER SIZE CLASS.
///
/// Budgeted by BYTES, not by count, and that is what makes it work. A leaf's
/// candidate list holds one whole-block prediction PER CANDIDATE at the same
/// time -- sixty-odd live buffers of the same size class -- so a flat depth of
/// 32 left the block-sized classes permanently empty and 461,428 requests fell
/// through to a fresh allocation. A byte budget gives the small classes the
/// depth they need while still capping what a thread can hold: one mebibyte
/// per class per element type, floor 4 buffers, ceiling 256.
///
/// Written with shifts rather than `(1 << 20) / ((1 << class) * elem_bytes)`
/// because `class` is a runtime value and the divisor is not a compile-time
/// constant. MEASURED NEUTRAL (490,156,038 vs 489,322,047 instructions on
/// 512x512 preset 10, i.e. inside the run-to-run spread) -- kept because it is
/// the honest form of the arithmetic, not because it bought anything.
#[cfg(feature = "std")]
const fn pool_depth(class: usize, elem_log2: u32) -> usize {
    let shift = class as u32 + elem_log2;
    if shift >= 20 {
        return 4;
    }
    let n = 1usize << (20 - shift);
    if n < 4 {
        4
    } else if n > 256 {
        256
    } else {
        n
    }
}

/// `ceil(log2(n))`, the class that serves a request for `n` elements.
#[cfg(feature = "std")]
#[inline]
fn class_for_request(n: usize) -> usize {
    if n <= 1 {
        return 0;
    }
    (usize::BITS - (n - 1).leading_zeros()) as usize
}

/// `floor(log2(cap))`, the class a buffer of capacity `cap` belongs to.
#[cfg(feature = "std")]
#[inline]
fn class_of_capacity(cap: usize) -> usize {
    (usize::BITS - 1 - cap.leading_zeros()) as usize
}

// MEASURED, do not "simplify" to a `Cell`: swapping the `RefCell` for a
// `Cell<Vec<Vec<T>>>` (take / pop / set, no borrow flag) was SLOWER --
// 489,282,103 instructions against 488,528,562 on 512x512 preset 10, pinned to
// one P-core. The borrow flag is cheaper than moving the list header twice.

#[cfg(feature = "std")]
std::thread_local! {
    static I32_POOL: core::cell::RefCell<[Vec<Vec<i32>>; CLASS_COUNT]> =
        const { core::cell::RefCell::new([const { Vec::new() }; CLASS_COUNT]) };
    static U8_POOL: core::cell::RefCell<[Vec<Vec<u8>>; CLASS_COUNT]> =
        const { core::cell::RefCell::new([const { Vec::new() }; CLASS_COUNT]) };
    static U16_POOL: core::cell::RefCell<[Vec<Vec<u16>>; CLASS_COUNT]> =
        const { core::cell::RefCell::new([const { Vec::new() }; CLASS_COUNT]) };
    static I16_POOL: core::cell::RefCell<[Vec<Vec<i16>>; CLASS_COUNT]> =
        const { core::cell::RefCell::new([const { Vec::new() }; CLASS_COUNT]) };
}

/// An element type that has a per-thread free list.
///
/// Sealed by being `pub(crate)`: the two implementors are the two buffers C
/// pools, and adding a third means adding a thread-local, not just an impl.
pub(crate) trait Pooled: Copy + Default + Sized + 'static {
    /// Take a parked buffer with capacity for at least `min_cap` elements, AS
    /// IT WAS PARKED -- its length and contents are whatever its last user
    /// left, not zero. When the class is empty this allocates the class size
    /// exactly, so the buffer it returns can be parked and re-used as-is.
    fn take_pooled(min_cap: usize) -> Vec<Self>;
    /// Park `v` in its size class if there is room, else drop it. Does NOT
    /// clear: keeping the high-water length is what lets `grown_out` re-use a
    /// buffer with no `memset` at all (see [`PoolVec::recycled_dirty`]).
    fn give_pooled(v: Vec<Self>);
}

macro_rules! impl_pooled {
    ($t:ty, $pool:ident) => {
        impl Pooled for $t {
            fn take_pooled(min_cap: usize) -> Vec<Self> {
                #[cfg(feature = "std")]
                {
                    let class = class_for_request(min_cap);
                    if class < CLASS_COUNT
                        && let Ok(Some(v)) = $pool.try_with(|p| p.borrow_mut()[class].pop())
                    {
                        return v;
                    }
                    // Allocate the CLASS size, not the request size, so this
                    // buffer lands back in the same class it was taken from
                    // and the next request of any size in that class fits it
                    // without a `realloc`.
                    if min_cap > 0 && class < CLASS_COUNT {
                        return Vec::with_capacity(1usize << class);
                    }
                }
                let _ = min_cap;
                Vec::new()
            }
            #[cfg_attr(feature = "std", allow(unused_mut))]
            fn give_pooled(mut v: Vec<Self>) {
                #[cfg(feature = "std")]
                {
                    let cap = v.capacity();
                    if cap == 0 {
                        return;
                    }
                    let class = class_of_capacity(cap);
                    if class >= CLASS_COUNT {
                        return;
                    }
                    let _ = $pool.try_with(|p| {
                        if let Ok(mut p) = p.try_borrow_mut()
                            && p[class].len()
                                < pool_depth(class, core::mem::size_of::<Self>().trailing_zeros())
                        {
                            p[class].push(v);
                        }
                    });
                }
                #[cfg(not(feature = "std"))]
                {
                    let _ = &mut v;
                }
            }
        }
    };
}

impl_pooled!(i32, I32_POOL);
impl_pooled!(u8, U8_POOL);
impl_pooled!(u16, U16_POOL);
impl_pooled!(i16, I16_POOL);

/// A `Vec<T>` that comes from, and returns to, its thread's free list.
///
/// Derefs to `Vec<T>`, so every read, index, `resize`, `truncate` and iterator
/// on the wrapped buffer is the `Vec` one. [`PoolVec::into_vec`] is the exit
/// to a plain owned `Vec` for the two places a coefficient buffer outlives
/// mode decision (`LeafDecision::{u,v}_qcoeffs`).
#[derive(Debug)]
pub(crate) struct PoolVec<T: Pooled>(Vec<T>);

impl<T: Pooled> PoolVec<T> {
    /// An empty buffer that has NOT touched the pool — the `const` initialiser
    /// the thread-local scratch structs need.
    pub(crate) const fn new() -> Self {
        PoolVec(Vec::new())
    }

    /// An EMPTY buffer with room for at least `min_cap` elements, recycled
    /// when the thread has one parked in that size class.
    ///
    /// `clear()` on a `Copy` element type is a length store, not a `memset`,
    /// so this costs nothing over [`Self::recycled_dirty`].
    pub(crate) fn pooled(min_cap: usize) -> Self {
        let mut v = T::take_pooled(min_cap);
        v.clear();
        PoolVec(v)
    }

    /// A recycled buffer WITHOUT clearing it: its length and contents are
    /// whatever its previous user left behind.
    ///
    /// This is the point of not clearing on the way in. `tx_pipeline`'s
    /// `grown_out` only ever `resize`s UP, so a buffer that comes back at its
    /// high-water length costs no `memset` at all -- where clearing first would
    /// have re-zeroed every element on every transform unit, which is the
    /// `vec![0; n]` cost this type exists to remove.
    ///
    /// ONLY for a consumer that provably writes every position it reads.
    /// [`crate::leaf_funnel::tx_pipeline::TxOutBufs`] carries that proof, per
    /// buffer and per quantizer path, and a release-build poisoning control
    /// (0x5A5A5A5A instead of zero) that left identity_full_8bit at 1100/1100.
    pub(crate) fn recycled_dirty(min_cap: usize) -> Self {
        PoolVec(T::take_pooled(min_cap))
    }

    /// Leave the pool: the buffer becomes an ordinary owned `Vec`.
    pub(crate) fn into_vec(mut self) -> Vec<T> {
        core::mem::take(&mut self.0)
    }

    /// A recycled copy of `src` — the pooled `to_vec()`.
    pub(crate) fn from_slice(src: &[T]) -> Self {
        let mut v = Self::pooled(src.len());
        v.0.extend_from_slice(src);
        v
    }
}

impl<T: Pooled> Clone for PoolVec<T> {
    /// A POOLED copy. `Cand` is `Clone` (the NIC staging pins clone winners),
    /// and cloning into a fresh `Vec` would put the allocation back exactly
    /// where this type removes it.
    fn clone(&self) -> Self {
        Self::from_slice(&self.0)
    }
}

impl<T: Pooled> FromIterator<T> for PoolVec<T> {
    /// `collect()` into a recycled buffer, so a `map().collect()` that used to
    /// allocate every time now allocates only when the free list is empty.
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let iter = iter.into_iter();
        let mut v = Self::pooled(iter.size_hint().0);
        v.0.extend(iter);
        v
    }
}

impl<T: Pooled> Default for PoolVec<T> {
    /// EMPTY and unpooled: a `Default` cannot know the size class it wants, and
    /// guessing one only shuffles the free list.
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Pooled> Deref for PoolVec<T> {
    type Target = Vec<T>;
    fn deref(&self) -> &Vec<T> {
        &self.0
    }
}

impl<T: Pooled> DerefMut for PoolVec<T> {
    fn deref_mut(&mut self) -> &mut Vec<T> {
        &mut self.0
    }
}

impl<T: Pooled> Drop for PoolVec<T> {
    fn drop(&mut self) {
        T::give_pooled(core::mem::take(&mut self.0));
    }
}

/// A pooled buffer of length `n` whose CONTENTS ARE UNSPECIFIED.
///
/// For a buffer whose very next statement is a PREDICTOR that writes every
/// position — `predict_unit`, `predict_unit_hbd`, `predict_intrabc_luma`, the
/// palette substitution loop. Those all define the whole `w * h` block, so the
/// zero fill is dead work.
///
/// It is worth its own function because the zero fill is NOT free once the
/// buffer is pooled: `vec![0; n]` reaches `calloc`, and glibc skips the
/// `memset` for a chunk carved from fresh kernel-zeroed pages, while a
/// recycled buffer always pays it. MEASURED: `__memset_avx2_unaligned_erms`
/// went from 2.96 % to 3.35 % of 512x512 preset-10 self time when these sites
/// moved to `zeroed_pool`.
///
/// SAFE, not merely fast: the length is real and every position is
/// initialised memory. The claim being made is about MEANING, not soundness —
/// reading one before the predictor writes it would give a stale sample rather
/// than undefined behaviour. VALIDATED the way `tx_pipeline::grown_out`
/// validates its own version of this claim: filling with 0x5A instead,
/// unconditionally in a release build, left identity_full_8bit at 1100/1100,
/// regression_spotcheck at 141/141, the screen palette gate at 50/50 and
/// screen_ibc_byte_gate at 152/152 — nothing reads an unwritten position.
pub(crate) fn dirty_pool<T: Pooled>(n: usize) -> PoolVec<T> {
    let mut v = PoolVec::recycled_dirty(n);
    if v.len() < n {
        v.resize(n, T::default());
    } else {
        v.truncate(n);
    }
    v
}

/// A pooled buffer of `n` zeros — the pooled `vec![T::default(); n]`.
///
/// MEASURED NEUTRAL: marking this and the free-list primitives `#[inline]`
/// moved 512x512 preset 10 from 490,229,561 to 490,393,427 instructions, i.e.
/// inside the run-to-run spread — LLVM was already inlining them. The
/// attributes are left OFF rather than kept as an unmeasured hint.
pub(crate) fn zeroed_pool<T: Pooled>(n: usize) -> PoolVec<T> {
    let mut v = PoolVec::pooled(n);
    v.resize(n, T::default());
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Empty every `i32` class so a test sees only its own buffers.
    fn drain_i32() {
        I32_POOL.with(|p| {
            for list in p.borrow_mut().iter_mut() {
                list.clear();
            }
        });
    }

    fn i32_class_len(class: usize) -> usize {
        I32_POOL.with(|p| p.borrow()[class].len())
    }

    /// The point of the type: a dropped buffer is the next one handed out, so
    /// a loop that allocates one buffer per iteration allocates ONCE.
    #[test]
    fn drop_parks_the_buffer_for_the_next_take() {
        drain_i32();
        let mut first = PoolVec::<i32>::pooled(1024);
        first.resize(1024, 0);
        let ptr = first.as_ptr() as usize;
        drop(first);
        let second = PoolVec::<i32>::pooled(1024);
        assert_eq!(second.as_ptr() as usize, ptr, "the parked buffer came back");
        assert_eq!(second.len(), 0, "`pooled` hands the buffer back EMPTY");
        assert!(
            second.capacity() >= 1024,
            "capacity survives the round trip"
        );
    }

    /// `recycled_dirty` is the other half: it must NOT clear, because
    /// `grown_out` relies on the high-water length to skip the zero fill.
    #[test]
    fn recycled_dirty_keeps_the_length() {
        drain_i32();
        let mut first = PoolVec::<i32>::pooled(64);
        first.resize(64, 7);
        drop(first);
        let second = PoolVec::<i32>::recycled_dirty(64);
        assert_eq!(second.len(), 64, "the length survives, unlike `pooled`");
        assert_eq!(second[0], 7, "and so do the bytes — this is the DIRTY take");
    }

    /// A request is served from a class every member of which is big enough.
    /// Without that, a 16-element buffer answers a 4096-element request and is
    /// immediately `realloc`ed — which is what a single flat free list did.
    #[test]
    fn a_small_buffer_never_answers_a_large_request() {
        drain_i32();
        let mut small = PoolVec::<i32>::pooled(16);
        small.resize(16, 0);
        drop(small);
        let big = PoolVec::<i32>::pooled(4096);
        assert!(
            big.capacity() >= 4096,
            "class {} must not serve a 4096-element request",
            class_for_request(16)
        );
        // The small buffer is still parked in ITS class, untouched.
        assert_eq!(i32_class_len(class_for_request(16)), 1);
    }

    /// `into_vec` is the exit: the buffer must NOT also be parked, or the pool
    /// would hand out a `Vec` that someone else still owns.
    #[test]
    fn into_vec_leaves_the_pool() {
        drain_i32();
        let mut v = PoolVec::<i32>::pooled(4);
        v.extend_from_slice(&[1, 2, 3]);
        let owned = v.into_vec();
        assert_eq!(owned, alloc::vec![1, 2, 3]);
        assert!(
            (0..CLASS_COUNT).all(|c| i32_class_len(c) == 0),
            "into_vec must not park the buffer it gave away"
        );
    }

    /// The per-class depth cap is what stops a thread that churns buffers from
    /// holding unbounded memory. It is a BYTE budget, so the cap depends on the
    /// class and the element size.
    #[test]
    fn each_class_is_capped_by_a_byte_budget() {
        drain_i32();
        let class = class_for_request(4);
        let depth = pool_depth(class, core::mem::size_of::<i32>().trailing_zeros());
        let mut held = alloc::vec::Vec::new();
        for _ in 0..(depth + 16) {
            let mut v = PoolVec::<i32>::pooled(4);
            v.resize(4, 0);
            held.push(v);
        }
        drop(held);
        assert_eq!(i32_class_len(class), depth);
        // One mebibyte per class per element type, floored at 4 and capped at
        // 256: a 4096-element `i32` class holds 64 buffers, not 256.
        assert_eq!(pool_depth(class_for_request(4096), 2), 64);
        assert_eq!(pool_depth(20, 2), 4, "huge classes keep the floor only");
    }
}
