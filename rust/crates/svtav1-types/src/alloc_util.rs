//! Fallible-allocation foundation (Feature 3 — foundation only).
//!
//! [`try_vec!`] is the drop-in replacement for `vec![val; len]` that becomes
//! fallible under the `fallible-alloc` feature and otherwise stays the fast
//! infallible `vec![..]` (which LLVM lowers to a single `calloc` for a
//! zero-fill). It always evaluates to `Result<Vec<_>, EncodeError>`, so a
//! call site can be written once and gain OOM-safety purely by flipping the
//! feature.
//!
//! FOUNDATION ONLY: no encoder call site is converted yet. The conversions
//! need `encode_frame_impl` to become fallible (which would collide with the
//! concurrent bd10 work), so they are deferred and listed in `DEFER.md` at
//! the workspace root.

/// Allocate a `Vec<T>` of `len` copies of `val`, returning
/// [`EncodeError::AllocFailed`](crate::error::EncodeError::AllocFailed)
/// instead of aborting the process when the reservation cannot be satisfied.
///
/// Only compiled under the `fallible-alloc` feature — the infallible default
/// path never references it.
#[cfg(feature = "fallible-alloc")]
pub fn alloc_vec_fallible<T: Clone>(
    len: usize,
    val: T,
) -> Result<alloc::vec::Vec<T>, crate::error::EncodeError> {
    let mut v = alloc::vec::Vec::new();
    v.try_reserve(len)
        .map_err(|_| crate::error::EncodeError::AllocFailed {
            requested_bytes: (len as u64).saturating_mul(core::mem::size_of::<T>() as u64),
            context: "",
        })?;
    v.resize(len, val);
    Ok(v)
}

/// Fallible `vec![val; len]`.
///
/// Always evaluates to `Result<Vec<_>, EncodeError>`:
/// - with `fallible-alloc`: routes through [`alloc_vec_fallible`], returning
///   `Err(EncodeError::AllocFailed)` on failure (no abort);
/// - without it: the infallible `Ok(vec![val; len])` fast path.
///
/// The `#[cfg]` is evaluated in the crate that *invokes* the macro, so the
/// arm selected follows that crate's `fallible-alloc` feature (the encoder
/// forwards it to `svtav1-types`).
#[macro_export]
macro_rules! try_vec {
    ($val:expr; $len:expr) => {{
        #[cfg(feature = "fallible-alloc")]
        {
            $crate::alloc_util::alloc_vec_fallible($len, $val)
        }
        #[cfg(not(feature = "fallible-alloc"))]
        {
            Ok::<::alloc::vec::Vec<_>, $crate::error::EncodeError>(::alloc::vec![$val; $len])
        }
    }};
}

#[cfg(test)]
mod tests {
    // A small request succeeds on both feature arms and yields the same vec
    // `vec![val; len]` would.
    #[test]
    fn try_vec_small_ok() {
        let r: Result<alloc::vec::Vec<u8>, crate::error::EncodeError> = try_vec![7u8; 4];
        assert_eq!(r.unwrap(), alloc::vec![7u8; 4]);
    }

    // Under `fallible-alloc`, an impossible request returns
    // `Err(AllocFailed)` rather than aborting the process.
    #[cfg(feature = "fallible-alloc")]
    #[test]
    fn try_vec_huge_is_alloc_failed() {
        let r = try_vec![0u8; usize::MAX];
        assert!(
            matches!(r, Err(crate::error::EncodeError::AllocFailed { .. })),
            "huge try_vec must be AllocFailed, got {r:?}"
        );
    }
}
