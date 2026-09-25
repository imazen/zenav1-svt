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
//! …) mostly stay where they are; the per-block ones are in `value_vars!`.
//!
//! Without `std` every flag reads `false` and every value `None`, so the
//! dumps compile to dead code (the crate root supplies an inert `eprintln!`).
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
    /// `SVTAV1_IFSDBG`: one line per MDS3 interpolation-filter search
    /// (`leaf_funnel::ifs`), joinable against C's `SVT_IFS_OUT`.
    ifsdbg => "SVTAV1_IFSDBG",
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
    /// `SVTAV1_SUBPEL`: per-(block, list, ref, stage) MD sub-pel search dump —
    /// the port-side twin of the C interposer's `SVT_SUBPEL_OUT` (one line per
    /// `svt_av1_find_best_sub_pixel_tree_pruned` call), printed with the same
    /// field names so the two join directly.
    subpeldbg => "SVTAV1_SUBPEL",
    /// `SVTAV1_REFSTATS`: the coded-area statistics this frame put on its DPB
    /// entry, in the field order of the C interposer's `REFSTATS` line
    /// (`tools/capture_c_trace/wrap_recon.c`) so the two can be joined
    /// directly. One line per coded frame.
    refstats => "SVTAV1_REFSTATS",
    /// `SVTAV1_CHAIN_DUMP`: funnel-chain dump.
    chain_dump => "SVTAV1_CHAIN_DUMP",
    /// `SVTAV1_SEED_DUMP`: funnel-seed dump.
    seed_dump => "SVTAV1_SEED_DUMP",
    /// `SVTAV1_RECONDBG`: post-deblock recon dump gate.
    recondbg => "SVTAV1_RECONDBG",
    /// `SVTAV1_DLFDBG`: per-trial deblock-level search dump (`DLF_TRY
    /// plane=<p> level=<l> sse=<v>` + `DLF_PICK plane=<p> level=<l>`).
    /// The C counterpart is `RECON_SSE` from the `--wrap` interposer for
    /// the level-0 trial; per-trial C values need the wrapped
    /// `svt_av1_loop_filter_frame` dump (tools/capture_c_trace).
    dlfdbg => "SVTAV1_DLFDBG",
    /// `SVTAV1_BD10_POSTPASS`: 10-bit post-pass gate.
    bd10_postpass => "SVTAV1_BD10_POSTPASS",
    /// `SVTAV1_LAMBDA_DBG`: per-superblock lambda derivation dump.
    lambda_dbg_set => "SVTAV1_LAMBDA_DBG",
    /// `SVTAV1_INTER_EXPERIMENTAL`: RETIRED. It last lifted the bit-depth
    /// floor on inter frames; that refusal is gone since 2026-09-18, when
    /// the `hbd_md = 2` MDS3 bump mirror closed the 10-bit recon drift and
    /// `bd10_video_selfcheck_gate.sh` measured 396/396 cells byte-identical
    /// to `aomdec` across the whole preset ladder. Nothing reads this
    /// variable now; harness scripts that still set it are harmless.
    ///
    /// HISTORY. It used to lift a blanket refusal of EVERY inter frame; on
    /// 2026-09-11 that became a preset floor while presets below 6 still
    /// drifted, and on 2026-09-15 the preset arm came off — the OBMC
    /// neighbour-prediction cache was serving one frame's predictions to
    /// the next (no frame identity in `NeighbourKey`), and once
    /// `obmc_pred_arm::begin_leaf` reset it per `evaluate_leaf`, the whole
    /// ladder decoded clean. What remained behind it until 2026-09-18 was
    /// the 10-bit inter measurement.
    ///
    /// MEASURED 2026-09-15, encoder recon vs `aomdec`, six public-domain derf
    /// clips x qp {20,40,55} x 8 frames: presets -1..6 and 13 are 162 of 162
    /// cells at 256x256 and presets -1..5 are 126 of 126 cells at 128x128
    /// with the temporal field ON, every frame byte-identical (presets 7..12
    /// were not re-swept — OBMC is off there and the standing
    /// video_selfcheck_gate claim covers them). The earlier 2026-09-11 text
    /// attributing the residual to the temporal MV field and a second
    /// spatial-stack defect was wrong about the second arm: the
    /// `SVTAV1_MFMV_OFF` failures were the same stale OBMC cache.
    ///
    /// Kept declared (dead code) so the retired variable stays greppable
    /// from this registry rather than only from history.
    #[allow(dead_code)]
    inter_experimental => "SVTAV1_INTER_EXPERIMENTAL",
    /// `SVTAV1_MFMV_OFF`: build the ref-MV stack from SPATIAL candidates only,
    /// and signal `use_ref_frame_mvs = 0` to match, so encoder and decoder
    /// agree.
    ///
    /// This is the ISOLATION SWITCH for the temporal motion-vector field. It
    /// did its job on 2026-09-10/11 by separating "the temporal MV field is
    /// wrong" from "something else is wrong"; both halves have since been
    /// fixed — the field itself (`sb64_sq_no4xn_geom` driving the simplified
    /// walk on rectangular blocks) and, on 2026-09-15, the residual low-preset
    /// drift, which turned out to be the OBMC neighbour-prediction cache
    /// holding one frame's predictions into the next, NOT the spatial stack
    /// the 2026-09-11 text blamed.
    ///
    /// Both halves must move together or the experiment is worthless: gating
    /// only the header desynchronises from frame 0, because the port builds
    /// the field unconditionally. That mistake was made first and is recorded
    /// so it is not repeated.
    ///
    /// **Not a feature flag.** C signals `use_ref_frame_mvs = 1` here, so a
    /// run with this set is NOT byte-comparable with C and must never be
    /// presented as a parity result.
    mfmv_off => "SVTAV1_MFMV_OFF",
    /// `SVTAV1_GM_EXPERIMENTAL`: lift the GLOBAL-MOTION refusal at presets
    /// 0..4 so the harness can MEASURE what an inter frame there emits.
    ///
    /// Same contract as `inter_experimental` above and for the same reason: a
    /// frame that emits is measurable and a frame that refuses is not. It is
    /// NOT a feature flag — the shipped refusal is what the public API does.
    gm_experimental => "SVTAV1_GM_EXPERIMENTAL",
    /// `SVTAV1_GMDBG`: print the port's frame-level global-motion derivation
    /// (`crate::port_global_me`) as one `GMPORT` line per inter frame, to be
    /// joined against C's `GMFRAME` line from the `SVT_GM_OUT` interposer.
    /// `tools/gm_join_gate.sh` is the join.
    gmdbg => "SVTAV1_GMDBG",
    /// `SVTAV1_PD0_NOSPLIT`: a **CONTROL**, not a configuration — force the
    /// video arm's PD0 to test only the 64x64 square on an INTER frame.
    ///
    /// C never runs this way. It exists to answer one question with a
    /// measurement instead of an argument: *is the inter frame's residual
    /// divergence in the MD/entropy path, or only in the PD0 partition?*
    /// Because it changes ONLY the inter frame, frame 0's recon — and
    /// therefore frame 1's reference — is untouched, so the comparison is
    /// clean.
    ///
    /// MEASURED 2026-09-02 on `diag 64x64 q40 p8 frames=2`: with it, frame 1
    /// is **byte-identical to C** (22 B) while the port's own PD0 splits the
    /// frame into sixteen 8x8 NEARESTMV blocks and emits 35 B — so the inter
    /// mode decision, the MV coding, the entropy path and the pack are all
    /// correct on that cell and the whole gap is PD0. See
    /// `docs/INTER-ENCODE-PLAN.md` §1z⁸.
    ///
    /// Delete it when PD0 does inter compensation; a byte count it produces is
    /// NEVER a parity result.
    pd0_nosplit => "SVTAV1_PD0_NOSPLIT",
    /// `SVTAV1_LPD1DBG`: per-superblock Light-PD1 dispatch dump — the
    /// `RLPD1-FRAME`/`RLPD1` lines reporting `pic_lpd1_lvl`, the resolved
    /// per-SB level, the PD0 root eval the detector read, and the
    /// post-detector level. Joins against the C `CLVL`/`CDET`/`CLPD1` lines
    /// the `SVT_LPD1DBG` interposer emits (reference enc_dec_process.c).
    lpd1dbg => "SVTAV1_LPD1DBG",
    /// `SVTAV1_TRELLIS`: per-coefficient trellis-decision dump inside
    /// `quant::optimize_b`. The hottest debug check in the encoder — the
    /// raw `var_os` was ~3.5 % of sampled encode cycles (2026-09-17,
    /// /tmp/perf_had.data, 24K samples).
    trellis => "SVTAV1_TRELLIS",
    /// `SVTAV1_RDOQDBG`: per-txb RDOQ context dump in `tx_pipeline`.
    rdoqdbg => "SVTAV1_RDOQDBG",
    /// `SVTAV1_SKIPDBG`: skip-decision dumps (`RSKIP`/`LSKIP`/`SKMDEC`/
    /// `SKMINJ` across mds1/mds3/light/port_md::inject).
    skipdbg => "SVTAV1_SKIPDBG",
    /// `SVTAV1_MRGDBG`: merge-candidate pruning dump
    /// (`inter_md_arm`, `leaf_funnel::nic`).
    mrgdbg => "SVTAV1_MRGDBG",
    /// `SVTAV1_MEDBG`: motion-estimation per-64x64 / per-SB dump
    /// (`inter_me_arm`, `pipeline`).
    medbg => "SVTAV1_MEDBG",
    /// `SVTAV1_COEFFDBG`: coefficient-context derivation dump (`pipeline`).
    coeffdbg => "SVTAV1_COEFFDBG",
    /// `SVTAV1_LAMDUMP`: per-superblock lambda-resolution dump (`pipeline`).
    lamdump => "SVTAV1_LAMDUMP",
    /// `SVTAV1_WINDBG`: per-block winner dump at `leaf_funnel::commit`.
    windbg => "SVTAV1_WINDBG",
    /// `SVTAV1_INTERDBG`: per-block inter-mode trace (`pipeline` `IDBG` lines).
    interdbg => "SVTAV1_INTERDBG",
    /// `SVTAV1_NSQSRCH`: NSQ full-pel motion-search trace
    /// (`port_md::md_search`).
    nsqsrch => "SVTAV1_NSQSRCH",
    /// `SVTAV1_PD0PRED`: PD0 winning-prediction pixel dump (`pd0.rs`); still
    /// ANDed behind [`pd0dbg`] at the call site.
    pd0pred => "SVTAV1_PD0PRED",
    /// `SVTAV1_QLEV_CO`: pre-quant coefficient dump; read only behind the
    /// `SVTAV1_QLEV_XY` pin in `mds3`.
    qlev_co => "SVTAV1_QLEV_CO",
    /// `SVTAV1_FHDBG`: frame-header field dump (`inter_hdr_arm`).
    fhdbg => "SVTAV1_FHDBG",
    /// `SVTAV1_LFDBG`: deblock-level dump (`pipeline` `LFDBG` line).
    lfdbg => "SVTAV1_LFDBG",
    /// `SVTAV1_RPSDBG`: reference-picture-set dump
    /// (`pipeline`, `port_picstruct`).
    rpsdbg => "SVTAV1_RPSDBG",
    /// `SVT_MFMV_DBG`: temporal-MV field dump (keeps the C-side `SVT_` name).
    mfmv_dbg => "SVT_MFMV_DBG",
    /// `ZZ_TPL`: TPL debug gate in `pipeline`.
    zz_tpl => "ZZ_TPL",
    /// `SVTAV1_DISPDBG`: per-frame TPL dispenser totals (`pipeline`).
    dispdbg => "SVTAV1_DISPDBG",
    /// `SVTAV1_QTRACE`: per-frame base qindex / layer line (`pipeline`).
    qtrace => "SVTAV1_QTRACE",
    /// `SVTAV1_BLKDBG`: per-block TPL dispenser dump (`port_tpl`).
    blkdbg => "SVTAV1_BLKDBG",
    /// `SVTAV1_CRDBG`: cyclic-refresh per-b64 decisions (`sb_qindex`).
    crdbg => "SVTAV1_CRDBG",
    /// `SVTAV1_PSYM`: partition-symbol stream trace, joinable against the C
    /// interposer's PSYM lines (`SVT_PARTSYM_OUT`).
    psym => "SVTAV1_PSYM",
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
    /// `SVTAV1_DLF_TRY_BIN=<prefix>`: dump the deblock-level search TRIAL
    /// plane for the level named by [`dlf_try_level`] to `<prefix>.p<plane>`,
    /// tightly packed. Pairs with the C `SVT_LFRECON_BIN` interposer
    /// (tools/capture_c_trace/wrap_recon.c) so the two deblock kernels can be
    /// diffed at ONE level on a byte-identical pre-filter recon.
    dlf_try_bin => "SVTAV1_DLF_TRY_BIN",
    /// `SVTAV1_DLF_TRY_LEVEL=<n>`: which trial [`dlf_try_bin`] captures.
    dlf_try_level => "SVTAV1_DLF_TRY_LEVEL",
    /// `SVTAV1_SC_TOOLS=nopalette|noibc|none`: bisect knob that removes
    /// palette and/or IntraBC from the MD candidate set WITHOUT touching the
    /// frame-header bit (`pipeline`). Unlike every other entry here it changes
    /// coding decisions; it is a debugging aid, not a configuration surface.
    sc_tools => "SVTAV1_SC_TOOLS",
}
