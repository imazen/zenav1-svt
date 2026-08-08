//! Cached lookups for the presence-only debug env vars.
//!
//! Every one of these gates a `eprintln!` diagnostic that is off in production,
//! but the *check* sat on per-block / per-candidate / per-txb paths, so the
//! encoder paid a real `getenv` per call. macOS `getenv` takes a lock
//! (`__findenv_locked`) and walks `environ` linearly, and profiling the port at
//! 512x512 measured that lock at **1.17 % of encode self time at preset 6 and
//! 1.47 % at preset 10** — i.e. ~1 % of wall clock spent proving that debug
//! logging is disabled.
//!
//! Each accessor resolves its variable exactly once into a `OnceLock<bool>`;
//! afterwards the check is one relaxed atomic load, which LLVM hoists out of
//! the block loops. This is the same pattern `leaf_funnel::dbg_on` and
//! `restoration::lr_dbg` already used — these accessors just finish the job for
//! the sites that were still calling `std::env::var_os` directly.
//!
//! Consequence, and it is deliberate: the variables are read **once per
//! process**, so setting one after the first encode has begun no longer takes
//! effect. Every caller is a stderr debug dump driven from a shell (`drill_cell.sh`,
//! `capture_c_trace`, the NSQDBG captures), which sets the variable before
//! launching the process, so nothing that exists loses a capability. Vars that
//! carry a *value* rather than presence (`SVTAV1_DBG_MI`, `SVTAV1_RECON_BIN`,
//! `SVTAV1_SC_TOOLS`, …) are unaffected and stay where they are.
//!
//! Bit-identity: these are pure read-side caches of a value the encoder already
//! read; no arithmetic and no coding decision changes. Pinned by
//! `tools/byteid_fingerprint.sh` (120/120 cells unchanged).

#[cfg(feature = "std")]
use std::sync::OnceLock;

/// Resolve `var`'s presence once, then answer from the cache.
#[cfg(feature = "std")]
#[inline]
fn once(cell: &'static OnceLock<bool>, var: &str) -> bool {
    *cell.get_or_init(|| std::env::var_os(var).is_some())
}

macro_rules! presence_flags {
    ($($(#[$m:meta])* $fn_name:ident => $var:literal),* $(,)?) => {
        $(
            $(#[$m])*
            #[cfg(feature = "std")]
            #[inline]
            pub(crate) fn $fn_name() -> bool {
                static CELL: OnceLock<bool> = OnceLock::new();
                once(&CELL, $var)
            }
            $(#[$m])*
            #[cfg(not(feature = "std"))]
            #[inline]
            pub(crate) fn $fn_name() -> bool { false }
        )*
    };
}

presence_flags! {
    /// `SVTAV1_NSQDBG`: MD-level non-square partition dump (per candidate).
    nsqdbg => "SVTAV1_NSQDBG",
    /// `SVTAV1_CANDDBG`: per-candidate cost dump inside the leaf funnel.
    canddbg => "SVTAV1_CANDDBG",
    /// `SVTAV1_PALBRK`: palette-decision breakdown (per block).
    palbrk => "SVTAV1_PALBRK",
    /// `SVTAV1_IBCDBG`: intra-block-copy candidate dump (per block).
    ibcdbg => "SVTAV1_IBCDBG",
    /// `SVTAV1_CDEF_DBG`: CDEF search mse rows (per filter block).
    cdef_dbg => "SVTAV1_CDEF_DBG",
    /// `SVTAV1_CODED_EOB`: per-txb coded-eob trace during packing.
    coded_eob => "SVTAV1_CODED_EOB",
    /// `SVTAV1_PACKTXB`: per-txb packing trace.
    packtxb => "SVTAV1_PACKTXB",
    /// `SVTAV1_TRACEMARK`: per-block packing marks.
    tracemark => "SVTAV1_TRACEMARK",
    /// `SVTAV1_BLKMARK`: per-block mark during packing.
    blkmark => "SVTAV1_BLKMARK",
    /// `SVTAV1_PART_DUMP`: partition-tree dump during packing.
    part_dump => "SVTAV1_PART_DUMP",
    /// `SVTAV1_DUMP_TREE`: whole partition tree dump.
    dump_tree => "SVTAV1_DUMP_TREE",
    /// `SVTAV1_DUMP_LR`: loop-restoration unit dump.
    dump_lr => "SVTAV1_DUMP_LR",
    /// `SVTAV1_PD0DBG`: PD0 decision dump (per block).
    pd0dbg => "SVTAV1_PD0DBG",
    /// `SVTAV1_CHAIN_DUMP`: funnel-chain dump.
    chain_dump => "SVTAV1_CHAIN_DUMP",
    /// `SVTAV1_SEED_DUMP`: funnel-seed dump.
    seed_dump => "SVTAV1_SEED_DUMP",
    /// `SVTAV1_RECONDBG`: post-deblock recon dump gate.
    recondbg => "SVTAV1_RECONDBG",
    /// `SVTAV1_BD10_POSTPASS`: 10-bit post-pass gate.
    bd10_postpass => "SVTAV1_BD10_POSTPASS",
    /// `SVTAV1_LAMBDA_DBG`: per-superblock lambda derivation dump.
    lambda_dbg_set => "SVTAV1_LAMBDA_DBG",
}

/// The value-carrying debug vars that also sit on per-block paths. Same
/// once-per-process contract as the presence flags; the cached `String` is
/// handed out by reference so callers do not re-allocate per block either.
macro_rules! value_vars {
    ($($(#[$m:meta])* $fn_name:ident => $var:literal),* $(,)?) => {
        $(
            $(#[$m])*
            #[cfg(feature = "std")]
            #[inline]
            pub(crate) fn $fn_name() -> Option<&'static str> {
                static CELL: OnceLock<Option<String>> = OnceLock::new();
                CELL.get_or_init(|| std::env::var($var).ok()).as_deref()
            }
            $(#[$m])*
            #[cfg(not(feature = "std"))]
            #[inline]
            pub(crate) fn $fn_name() -> Option<&'static str> { None }
        )*
    };
}

value_vars! {
    /// `SVTAV1_PACKTREE=<path>`: append one line per coded leaf to that file.
    packtree => "SVTAV1_PACKTREE",
    /// `SVTAV1_PACKTREE_COEFF`: `"mi_row,mi_col"` pin, or a path for all leaves.
    packtree_coeff => "SVTAV1_PACKTREE_COEFF",
}
