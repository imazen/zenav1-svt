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

/// How many buffers of each type one thread keeps parked.
///
/// Deep enough for the whole live set of a transform unit (two `TxtScratch`
/// slots, the owned wrapper's pair, the chroma pair and the CfL pair, plus the
/// per-TXB `dep_q` row of the largest block), shallow enough that a thread that
/// stops encoding gives the memory back promptly.
#[cfg(feature = "std")]
const POOL_DEPTH: usize = 64;

// MEASURED, do not "simplify" to a `Cell`: swapping the `RefCell` for a
// `Cell<Vec<Vec<T>>>` (take / pop / set, no borrow flag) was SLOWER --
// 489,282,103 instructions against 488,528,562 on 512x512 preset 10, pinned to
// one P-core. The borrow flag is cheaper than moving the list header twice.

#[cfg(feature = "std")]
std::thread_local! {
    static I32_POOL: core::cell::RefCell<Vec<Vec<i32>>> =
        const { core::cell::RefCell::new(Vec::new()) };
    static U8_POOL: core::cell::RefCell<Vec<Vec<u8>>> =
        const { core::cell::RefCell::new(Vec::new()) };
    static U16_POOL: core::cell::RefCell<Vec<Vec<u16>>> =
        const { core::cell::RefCell::new(Vec::new()) };
}

/// An element type that has a per-thread free list.
///
/// Sealed by being `pub(crate)`: the two implementors are the two buffers C
/// pools, and adding a third means adding a thread-local, not just an impl.
pub(crate) trait Pooled: Copy + Default + Sized + 'static {
    /// Take a parked buffer AS IT WAS PARKED -- its length and contents are
    /// whatever its last user left, not zero. [`PoolVec::pooled`] clears it;
    /// [`PoolVec::recycled_dirty`] deliberately does not.
    fn take_pooled() -> Vec<Self>;
    /// Park `v` if there is room, else drop it. Does NOT clear: keeping the
    /// high-water length is what lets `grown_out` re-use a buffer with no
    /// `memset` at all (see [`PoolVec::recycled_dirty`]).
    fn give_pooled(v: Vec<Self>);
}

macro_rules! impl_pooled {
    ($t:ty, $pool:ident) => {
        impl Pooled for $t {
            fn take_pooled() -> Vec<Self> {
                #[cfg(feature = "std")]
                {
                    if let Ok(Some(v)) = $pool.try_with(|p| p.borrow_mut().pop()) {
                        return v;
                    }
                }
                Vec::new()
            }
            fn give_pooled(mut v: Vec<Self>) {
                #[cfg(feature = "std")]
                {
                    if v.capacity() == 0 {
                        return;
                    }
                    let _ = $pool.try_with(|p| {
                        if let Ok(mut p) = p.try_borrow_mut()
                            && p.len() < POOL_DEPTH
                        {
                            p.push(v);
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

    /// An EMPTY buffer, recycled when the thread has one parked.
    ///
    /// `clear()` on a `Copy` element type is a length store, not a `memset`,
    /// so this costs nothing over [`Self::recycled_dirty`].
    pub(crate) fn pooled() -> Self {
        let mut v = T::take_pooled();
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
    pub(crate) fn recycled_dirty() -> Self {
        PoolVec(T::take_pooled())
    }

    /// Leave the pool: the buffer becomes an ordinary owned `Vec`.
    pub(crate) fn into_vec(mut self) -> Vec<T> {
        core::mem::take(&mut self.0)
    }

    /// A recycled copy of `src` — the pooled `to_vec()`.
    pub(crate) fn from_slice(src: &[T]) -> Self {
        let mut v = Self::pooled();
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
        let mut v = Self::pooled();
        v.0.extend(iter);
        v
    }
}

impl<T: Pooled> Default for PoolVec<T> {
    fn default() -> Self {
        Self::pooled()
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

/// A pooled buffer of `n` zeros — the pooled `vec![T::default(); n]`.
pub(crate) fn zeroed_pool<T: Pooled>(n: usize) -> PoolVec<T> {
    let mut v = PoolVec::pooled();
    v.resize(n, T::default());
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The point of the type: a dropped buffer is the next one handed out, so
    /// a loop that allocates one buffer per iteration allocates ONCE.
    #[test]
    fn drop_parks_the_buffer_for_the_next_take() {
        // Drain whatever this thread already parked so the test sees its own
        // buffers, not another test's.
        while I32_POOL.with(|p| p.borrow_mut().pop()).is_some() {}
        let mut first = PoolVec::<i32>::pooled();
        first.resize(1024, 0);
        let ptr = first.as_ptr() as usize;
        drop(first);
        let second = PoolVec::<i32>::pooled();
        assert_eq!(second.as_ptr() as usize, ptr, "the parked buffer came back");
        assert_eq!(second.len(), 0, "parked buffers come back EMPTY");
        assert!(
            second.capacity() >= 1024,
            "capacity survives the round trip"
        );
    }

    /// `into_vec` is the exit: the buffer must NOT also be parked, or the pool
    /// would hand out a `Vec` that someone else still owns.
    #[test]
    fn into_vec_leaves_the_pool() {
        while I32_POOL.with(|p| p.borrow_mut().pop()).is_some() {}
        let mut v = PoolVec::<i32>::pooled();
        v.extend_from_slice(&[1, 2, 3]);
        let owned = v.into_vec();
        assert_eq!(owned, alloc::vec![1, 2, 3]);
        assert_eq!(
            I32_POOL.with(|p| p.borrow().len()),
            0,
            "into_vec must not park the buffer it gave away"
        );
    }

    /// The depth cap is what stops a thread that churns buffers from holding
    /// unbounded memory.
    #[test]
    fn the_pool_is_capped() {
        while I32_POOL.with(|p| p.borrow_mut().pop()).is_some() {}
        let mut held = alloc::vec::Vec::new();
        for _ in 0..(POOL_DEPTH + 16) {
            let mut v = PoolVec::<i32>::pooled();
            v.resize(4, 0);
            held.push(v);
        }
        drop(held);
        assert_eq!(I32_POOL.with(|p| p.borrow().len()), POOL_DEPTH);
    }
}
