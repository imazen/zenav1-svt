//! Build script: compiles the C shims and links the in-tree C SVT-AV1 static
//! library so tests can compare Rust output against the reference bit-for-bit.
//!
//! The C reference is CARGO-DRIVEN (issue #4, invariant B): on a fresh clone
//! `cargo test` configures and builds the C library itself — no manual cmake
//! step — for BOTH oracles the differential tooling compares against:
//!
//! | variant  | cmake flag          | lib dir            | build dir           |
//! |----------|---------------------|--------------------|---------------------|
//! | mainline | `-DSVT_HDR_MODE=OFF`| `<repo>/Bin/Release`    | `<repo>/cbuild-static`     |
//! | fork     | `-DSVT_HDR_MODE=ON` | `<repo>/Bin/ReleaseHdr` | `<repo>/cbuild-static-hdr` |
//!
//! The shims link the MAINLINE lib (every `c_parity_*` test, fork features
//! included — the fork's exported kernels are compiled into both variants).
//! The fork lib is what `tools/capture_c_trace/build.sh` links under
//! `SVT_HDR_MODE=1` for the byte-vs-fork gates (`tools/hdr_bd10_gate.sh`);
//! building it here is what makes those gates work on a fresh box without a
//! hand-typed cmake line. The dirs/flags above are the ones every shell tool
//! already assumes (`capture_c_trace/build.sh`, `.github/workflows/rust-gates.yml`),
//! so a build produced here and one produced by hand are interchangeable.
//!
//! Caching (invariant C): each lib dir carries a stamp file
//! (`.zenav1-cref-stamp`) recording the C submodule's git SHA and the config
//! key. An unchanged tree never rebuilds — the script re-runs only when the
//! stamp, the submodule HEAD, or an env knob changes, and even then
//! `cmake --build` is incremental. First build is minutes, once.
//!
//! Env knobs (all optional):
//! * `SVT_CREF_LIB_DIR` — use this directory's `libSvtAv1Enc.a` as the mainline
//!   oracle and build NOTHING (the caller's own artifact; never written to).
//! * `SVT_CREF_SKIP_HDR=1` — skip the fork variant (it is not linked by cargo
//!   tests; only the shell gates need it).
//! * `SVT_CREF_JOBS` — parallelism for `cmake --build` (default: cargo's
//!   `NUM_JOBS`, i.e. `cargo -j N`).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const STAMP_FILE: &str = ".zenav1-cref-stamp";
/// Bump when the cmake flags below change so a stale build dir is rebuilt.
const CONFIG_VERSION: &str = "v1";

struct Variant {
    name: &'static str,
    hdr_mode: bool,
    build_apps: bool,
    build_dir: PathBuf,
    lib_dir: PathBuf,
}

impl Variant {
    fn config_key(&self) -> String {
        format!(
            "{CONFIG_VERSION}:hdr={},apps={},lto=off,native=off,shared=off,testing=off,type=Release",
            if self.hdr_mode { "on" } else { "off" },
            if self.build_apps { "on" } else { "off" },
        )
    }
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    // crates/svtav1-cref -> crates -> rust -> repo root
    let repo_root = manifest
        .ancestors()
        .nth(3)
        .expect("svtav1-cref must live at <repo>/rust/crates/svtav1-cref")
        .to_path_buf();

    // The C reference tree is the `reference/svt-av1` submodule
    // (imazen/svt-av1-ref: SVT-AV1 v4.2.0 + gated SVT_HDR_MODE).
    let c_root = repo_root.join("reference/svt-av1");
    if !c_root.join("Source").exists() {
        panic!(
            "C reference submodule missing at {}.\n\
             Run: git submodule update --init",
            c_root.display(),
        );
    }

    println!("cargo:rerun-if-env-changed=SVT_CREF_LIB_DIR");
    println!("cargo:rerun-if-env-changed=SVT_CREF_SKIP_HDR");
    println!("cargo:rerun-if-env-changed=SVT_CREF_JOBS");
    println!("cargo:rerun-if-changed=shims/ref_shims.c");
    println!("cargo:rerun-if-changed=shims/inter_mvp_shims.c");
    println!("cargo:rerun-if-changed=shims/inter_me_shims.c");
    println!("cargo:rerun-if-changed=shims/txfm_pf_shims.c");
    println!("cargo:rerun-if-changed=shims/inv_recon_shims.c");
    println!("cargo:rerun-if-changed=shims/picstruct_shims.c");
    println!("cargo:rerun-if-changed=shims/md_subpel_shims.c");
    println!("cargo:rerun-if-changed=shims/inter_pred_shims.c");
    println!("cargo:rerun-if-changed=shims/rc_shims.c");
    println!("cargo:rerun-if-changed=shims/sigderiv_shims.c");
    println!("cargo:rerun-if-changed=shims/preanalysis_shims.c");
    println!("cargo:rerun-if-changed=shims/tf_shims.c");
    println!("cargo:rerun-if-changed=shims/mode_decision_shims.c");
    println!("cargo:rerun-if-changed=shims/entropy_inter_shims.c");
    println!("cargo:rerun-if-changed=shims/interpred_gap_shims.c");
    println!("cargo:rerun-if-changed=shims/pcl_shims.c");
    println!("cargo:rerun-if-changed=shims/picops_dblk_shims.c");
    println!("cargo:rerun-if-changed=shims/entropy_block_shims.c");
    println!("cargo:rerun-if-changed=shims/rc_vbr_cbr_shims.c");
    println!("cargo:rerun-if-changed=shims/rd_cost_shims.c");
    println!("cargo:rerun-if-changed=shims/full_loop_md_shims.c");
    println!("cargo:rerun-if-changed=shims/md_winner_shims.c");
    // The submodule's checked-out commit: when it moves, the oracle must be
    // rebuilt. (`reference/svt-av1/.git` is a gitdir pointer into the parent's
    // `.git/modules/…`; HEAD there is the file that changes on checkout.)
    let submodule_head = repo_root.join(".git/modules/reference/svt-av1/HEAD");
    if submodule_head.exists() {
        println!("cargo:rerun-if-changed={}", submodule_head.display());
    }

    let lib_dir = match env::var_os("SVT_CREF_LIB_DIR") {
        Some(dir) => {
            // The caller's own artifact: link it, never build into it.
            let dir = PathBuf::from(dir);
            let archive = dir.join("libSvtAv1Enc.a");
            if !archive.exists() {
                panic!(
                    "SVT_CREF_LIB_DIR is set but {} does not exist. Unset it to let this \
                     build script build the C reference, or point it at a directory that \
                     holds libSvtAv1Enc.a.",
                    archive.display()
                );
            }
            dir
        }
        None => {
            let sha = submodule_sha(&c_root);
            let mainline = Variant {
                name: "mainline (SVT_HDR_MODE=OFF)",
                hdr_mode: false,
                // The shell gates (`sb128_gate.sh`, `unaligned_identity_scan.sh`)
                // run SvtAv1EncApp from this dir, so build it with the lib —
                // the same config CI and the hand-typed line have always used.
                build_apps: true,
                build_dir: repo_root.join("cbuild-static"),
                lib_dir: repo_root.join("Bin/Release"),
            };
            ensure_variant(&c_root, &mainline, sha.as_deref());
            if env::var("SVT_CREF_SKIP_HDR")
                .map(|v| v == "1")
                .unwrap_or(false)
            {
                println!(
                    "cargo:warning=SVT_CREF_SKIP_HDR=1: the fork oracle (Bin/ReleaseHdr) was not \
                     built; tools/hdr_bd10_gate.sh will need it"
                );
            } else {
                let fork = Variant {
                    name: "fork (SVT_HDR_MODE=ON)",
                    hdr_mode: true,
                    build_apps: false,
                    build_dir: repo_root.join("cbuild-static-hdr"),
                    lib_dir: repo_root.join("Bin/ReleaseHdr"),
                };
                ensure_variant(&c_root, &fork, sha.as_deref());
            }
            mainline.lib_dir
        }
    };

    // Promote three `static` pd_process.c functions to linkable symbols so the
    // wp-picstruct differential can reach them at evidence tier 1. Runs BEFORE
    // the shim compile because the shim's tier-1 entry points are behind the
    // define this returns. See the function for the whole rationale and the
    // failure modes it deliberately tolerates.
    let picstruct_statics = link_globalized_pd_statics(&repo_root, &out_dir_path());
    // Same mechanism for `rc_vbr_cbr.c`'s five surviving statics (lane wx-rc).
    let rc_vbr_statics = link_globalized_rc_vbr_statics(&repo_root, &out_dir_path());

    let mut shims = cc::Build::new();
    if picstruct_statics {
        shims.define("SVTAV1_CREF_PICSTRUCT_STATICS", "1");
    }
    if rc_vbr_statics {
        shims.define("SVTAV1_CREF_RC_VBR_STATICS", "1");
    }
    shims
        .file(manifest.join("shims/ref_shims.c"))
        // Inter MVP oracle (chunk C2) — its own TU so the C2 and C3 lanes
        // never share a shim file in one working copy.
        .file(manifest.join("shims/inter_mvp_shims.c"))
        .file(manifest.join("shims/inter_me_shims.c"))
        // Reduced-coefficient-shape transforms (wp-transforms lane).
        .file(manifest.join("shims/txfm_pf_shims.c"))
        .file(manifest.join("shims/inv_recon_shims.c"))
        // mcomp.c sub-pel tree oracle (lane wp-search) — its own TU for the
        // same per-lane-file-ownership reason as the two above.
        .file(manifest.join("shims/md_subpel_shims.c"))
        // pd_process.c picture-decision oracle (lane wp-picstruct) — same.
        .file(manifest.join("shims/picstruct_shims.c"))
        // Inter prediction / MC oracle (wholesale inter_prediction.c lane).
        .file(manifest.join("shims/inter_pred_shims.c"))
        // Rate control oracle (lane wp-ratecontrol) — own TU, same reason.
        .file(manifest.join("shims/rc_shims.c"))
        // enc_mode_config.c signal-derivation oracle (wp-sigderiv lane).
        .file(manifest.join("shims/sigderiv_shims.c"))
        // Pre-analysis oracle (temporal filtering / noise model / source stats).
        .file(manifest.join("shims/preanalysis_shims.c"))
        // temporal_filtering.c oracle (lane wp-preanalysis) — own TU, same reason.
        .file(manifest.join("shims/tf_shims.c"))
        // Mode-decision oracle (lane wp-modedecision) — likewise its own TU.
        .file(manifest.join("shims/mode_decision_shims.c"))
        // Inter bitstream-syntax oracle (entropy_coding.c inter group) — its
        // own TU for the same per-lane file-ownership reason.
        .file(manifest.join("shims/entropy_inter_shims.c"))
        // The inter-prediction functions the wholesale-MC lane left unported
        // (C_DEFAULT/inter_prediction_c.c, the 10-bit light-PD1 arm) — lane
        // wx-interpred, own TU for the same file-ownership reason.
        .file(manifest.join("shims/interpred_gap_shims.c"))
        // product_coding_loop.c candidate-staging oracle (lane wx-pcl) — own TU.
        .file(manifest.join("shims/pcl_shims.c"))
        // pic_operators.c / deblocking_common.c / intra_prediction.c residual
        // oracle (lane wx-intra-dblk) — own TU, same reason.
        .file(manifest.join("shims/picops_dblk_shims.c"))
        // Per-block emission oracle (write_modes_b / write_modes_sb group) —
        // its own TU, same per-lane file-ownership reason.
        .file(manifest.join("shims/entropy_block_shims.c"))
        // rc_vbr_cbr.c VBR/CBR state machine oracle (lane wx-rc) — own TU.
        .file(manifest.join("shims/rc_vbr_cbr_shims.c"))
        // rd_cost.c MD-cost oracle (lane wx-md) — its own TU for the same
        // per-lane file-ownership reason as the others above.
        .file(manifest.join("shims/rd_cost_shims.c"))
        // full_loop.c MD-side oracle (lane wx-md) — the same lane's second
        // C file, kept in its own TU so the two never collide.
        .file(manifest.join("shims/full_loop_md_shims.c"))
        // mode_decision.c full-mode-decision oracle (lane wx-md) — own TU.
        .file(manifest.join("shims/md_winner_shims.c"))
        .include(c_root.join("Source/Lib/Codec"))
        .include(c_root.join("Source/API"))
        .include(c_root.join("Source/Lib/Globals"))
        .include(c_root.join("Source/Lib/C_DEFAULT"))
        .warnings(false)
        .compile("svtav1_cref_shims");

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=SvtAv1Enc");
    println!("cargo:rustc-link-lib=pthread");
    println!("cargo:rustc-link-lib=m");
}

fn out_dir_path() -> PathBuf {
    PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is always set for a build script"))
}

/// Make `set_ref_list_counts` and `set_all_ref_frame_type` linkable.
///
/// Both are `static` in `Codec/pd_process.c`, so `nm -g` on
/// `libSvtAv1Enc.a` does not find them and a differential against the real C
/// code would be impossible — the port would be stuck at evidence tier 4 for
/// two of the highest-value functions in the picture-decision group
/// (`docs/WORKING-ON-THIS.md` §4). They DO survive in the CMake object file as
/// local (`t`) symbols, so `llvm-objcopy --globalize-symbol` on a PRIVATE COPY
/// of that object promotes them without touching the C tree or the archive.
///
/// Linking the promoted object alongside the archive does NOT produce
/// duplicate symbols: the object supplies every symbol `pd_process.c.o` would
/// have, so the archive member is never pulled in. Verified on macOS arm64
/// before this was wired up (a standalone `cc probe.c globalized.o
/// libSvtAv1Enc.a` links and the three addresses resolve).
///
/// **This is best-effort and MUST stay best-effort.** The object exists only
/// after the C library has been built into `<repo>/cbuild-static`, which does
/// not happen when the caller points `SVT_CREF_LIB_DIR` at a prebuilt archive,
/// and `llvm-objcopy` is not on every host. When either is missing this
/// function emits a `cargo:warning` naming exactly what is unavailable and
/// returns; the `picstruct_statics` cfg stays off and the tier-1 tests that
/// need it do not compile.
///
/// That is a skip, so per the project's no-silent-skip rule the DECISION is
/// the caller's, not the test's: set
/// `SVT_CREF_REQUIRE_PICSTRUCT_STATICS=1` and
/// `picstruct_statics_oracle_is_available` fails loudly instead. CI can turn
/// that on once the object is known to be present on its image.
#[must_use]
fn link_globalized_pd_statics(repo_root: &Path, out_dir: &Path) -> bool {
    println!("cargo:rustc-check-cfg=cfg(picstruct_statics)");
    println!("cargo:rerun-if-env-changed=SVT_CREF_REQUIRE_PICSTRUCT_STATICS");
    println!("cargo:rerun-if-env-changed=LLVM_OBJCOPY");

    // ONLY functions whose compiled ABI has been checked against the source
    // signature belong here. Globalizing makes a symbol linkable; it does NOT
    // make the declared signature right. `scene_transition_detector` is the
    // counterexample and is deliberately absent: LLVM promoted its
    // `PictureParentControlSet** window` parameter to the current PPCS, so
    // calling it as declared segfaults. Disassemble the prologue before adding
    // a name here (see shims/picstruct_shims.c for the two checks that passed).
    const SYMS: [&str; 2] = ["set_ref_list_counts", "set_all_ref_frame_type"];

    let src = repo_root.join("cbuild-static/Source/Lib/Codec/CMakeFiles/CODEC.dir/pd_process.c.o");
    println!("cargo:rerun-if-changed={}", src.display());
    if !src.exists() {
        println!(
            "cargo:warning=picstruct tier-1 statics unavailable: {} not found (the C library              has not been built into <repo>/cbuild-static on this host).              set_ref_list_counts / set_all_ref_frame_type stay at              evidence tier 4.",
            src.display()
        );
        return false;
    }

    let Some(objcopy) = find_objcopy() else {
        println!(
            "cargo:warning=picstruct tier-1 statics unavailable: no llvm-objcopy found (tried              $LLVM_OBJCOPY, llvm-objcopy, objcopy, /opt/homebrew/opt/llvm/bin/llvm-objcopy).              set_ref_list_counts / set_all_ref_frame_type stay at              evidence tier 4."
        );
        return false;
    };

    let dst = out_dir.join("pd_process_globalized.o");
    if fs::copy(&src, &dst).is_err() {
        println!("cargo:warning=picstruct tier-1 statics unavailable: could not copy the object");
        return false;
    }
    // Mach-O prefixes C symbols with an underscore; ELF does not. Pass both
    // spellings and let objcopy ignore the one that does not match.
    let mut cmd = Command::new(&objcopy);
    for s in SYMS {
        cmd.arg(format!("--globalize-symbol={s}"));
        cmd.arg(format!("--globalize-symbol=_{s}"));
    }
    cmd.arg(&dst);
    match cmd.status() {
        Ok(st) if st.success() => {}
        other => {
            println!(
                "cargo:warning=picstruct tier-1 statics unavailable: {} failed ({other:?})",
                objcopy.display()
            );
            return false;
        }
    }

    // Wrap the object in an archive rather than emitting `rustc-link-arg`:
    // a link-arg applies only to THIS crate's own link, while
    // `rustc-link-lib` + `rustc-link-search` propagate to every dependent
    // binary — which is where the differential test actually links. Measured:
    // with the link-arg form the encoder's test binary failed with
    // "Undefined symbols: _set_ref_list_counts, _set_all_ref_frame_type".
    //
    // Archive ORDER matters: this one is emitted BEFORE SvtAv1Enc, so the
    // shim pulls this member (defining every pd_process.c symbol) and the
    // archive's own pd_process.c.o member is then never pulled — which is why
    // there is no duplicate-symbol error.
    let archive = out_dir.join("libpd_statics.a");
    let _ = fs::remove_file(&archive);
    match Command::new("ar")
        .arg("crs")
        .arg(&archive)
        .arg(&dst)
        .status()
    {
        Ok(st) if st.success() => {}
        other => {
            println!(
                "cargo:warning=picstruct tier-1 statics unavailable: `ar crs` failed ({other:?})"
            );
            return false;
        }
    }
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=pd_statics");
    println!("cargo:rustc-cfg=picstruct_statics");
    // The shim TU compiles its tier-1 entry points only when the promotion
    // succeeded, so the crate still builds on a host without the object.
    true
}

/// Make `rc_vbr_cbr.c`'s five surviving `static` functions linkable.
///
/// Identical mechanism to [`link_globalized_pd_statics`] (read its doc comment
/// for the rationale, the archive-ordering argument and the failure modes):
/// copy the CMake object, `--globalize-symbol` the names on the COPY, wrap it
/// in an archive emitted BEFORE `libSvtAv1Enc.a` so the archive's own
/// `rc_vbr_cbr.c.o` member is never pulled.
///
/// **Only these five names, and only because they survived AND kept their
/// source ABI.** `nm` on
/// `cbuild-static/.../rc_vbr_cbr.c.o` shows the file's other ~40 statics were
/// inlined away by the Release build and have no symbol at any linkage, so
/// they stay at evidence tier 4 and no amount of objcopy changes that. The
/// signature each shim declares is transcribed from the C DEFINITION;
/// globalizing makes a symbol linkable but does NOT check its signature, so
/// every one of the five was read at its line number before being listed.
///
/// Best-effort for the same two reasons as the pd variant: the object exists
/// only after a `<repo>/cbuild-static` build (not when `SVT_CREF_LIB_DIR`
/// points at a prebuilt archive), and `llvm-objcopy` is not on every host.
/// A miss emits a `cargo:warning`, leaves `rc_vbr_statics` off, and the tier-1
/// tests do not compile — which per the project's no-silent-skip rule is a
/// decision the CALLER makes: `SVT_CREF_REQUIRE_RC_VBR_STATICS=1` makes
/// `rc_vbr_statics_oracle_is_available` fail loudly instead.
#[must_use]
fn link_globalized_rc_vbr_statics(repo_root: &Path, out_dir: &Path) -> bool {
    println!("cargo:rustc-check-cfg=cfg(rc_vbr_statics)");
    println!("cargo:rerun-if-env-changed=SVT_CREF_REQUIRE_RC_VBR_STATICS");

    // Every name here had its PROLOGUE DISASSEMBLED against the source
    // signature before being added (`otool -tV rc_vbr_cbr.c.o`, macOS arm64,
    // 2026-08-31). Globalizing makes a symbol linkable; it does NOT make the
    // declared signature right, and a wrong one corrupts the stack instead of
    // failing to link.
    //
    // DELIBERATELY ABSENT, and this is the interesting entry:
    // `calc_active_worst_quality_no_stats_cbr` HAS a surviving `t` symbol but
    // LLVM SPECIALIZED ITS ABI. Its source signature is
    // `(PictureParentControlSet*)`; the compiled prologue takes TWO arguments
    // and `x0` is not a PPCS — it reads `[x0,#0x2480]`/`[x0,#0x2484]` for
    // `avg_frame_qindex[0..1]` and branches on `cbz w1` for the
    // `frame_type == KEY_FRAME` early return, i.e. the frame-type test was
    // hoisted into a caller-supplied flag and the pointer was narrowed past
    // the PPCS. Calling it as declared returned 0 for every input in a first
    // draft of `c_parity_rc_vbr_cbr_state.rs`. Same failure class as
    // `scene_transition_detector` in `link_globalized_pd_statics`. It is
    // reached INDIRECTLY instead, through the exported
    // `svt_av1_resize_reset_rc`, which calls it — see that test.
    const SYMS: [&str; 5] = [
        "av1_rc_regulate_q",
        "av1_rc_update_rate_correction_factors",
        "get_regulated_q_overshoot",
        "get_regulated_q_undershoot",
        // 2-arg `(SequenceControlSet*, int)`, ABI unchanged (verified).
        "clamp_qindex",
    ];

    let src = repo_root.join("cbuild-static/Source/Lib/Codec/CMakeFiles/CODEC.dir/rc_vbr_cbr.c.o");
    println!("cargo:rerun-if-changed={}", src.display());
    if !src.exists() {
        println!(
            "cargo:warning=rc_vbr_cbr tier-1 statics unavailable: {} not found (the C library \
             has not been built into <repo>/cbuild-static on this host). av1_rc_regulate_q and \
             friends stay at evidence tier 4.",
            src.display()
        );
        return false;
    }

    let Some(objcopy) = find_objcopy() else {
        println!(
            "cargo:warning=rc_vbr_cbr tier-1 statics unavailable: no llvm-objcopy found. \
             av1_rc_regulate_q and friends stay at evidence tier 4."
        );
        return false;
    };

    let dst = out_dir.join("rc_vbr_cbr_globalized.o");
    if fs::copy(&src, &dst).is_err() {
        println!("cargo:warning=rc_vbr_cbr tier-1 statics unavailable: could not copy the object");
        return false;
    }
    // Mach-O prefixes C symbols with an underscore; ELF does not. Pass both
    // spellings and let objcopy ignore the one that does not match.
    let mut cmd = Command::new(&objcopy);
    for s in SYMS {
        cmd.arg(format!("--globalize-symbol={s}"));
        cmd.arg(format!("--globalize-symbol=_{s}"));
    }
    cmd.arg(&dst);
    match cmd.status() {
        Ok(st) if st.success() => {}
        other => {
            println!(
                "cargo:warning=rc_vbr_cbr tier-1 statics unavailable: {} failed ({other:?})",
                objcopy.display()
            );
            return false;
        }
    }

    let archive = out_dir.join("librc_vbr_statics.a");
    let _ = fs::remove_file(&archive);
    match Command::new("ar")
        .arg("crs")
        .arg(&archive)
        .arg(&dst)
        .status()
    {
        Ok(st) if st.success() => {}
        other => {
            println!(
                "cargo:warning=rc_vbr_cbr tier-1 statics unavailable: `ar crs` failed ({other:?})"
            );
            return false;
        }
    }
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=rc_vbr_statics");
    println!("cargo:rustc-cfg=rc_vbr_statics");
    true
}

/// First working objcopy, or `None`. `--version` is the probe because a name
/// on PATH is not proof it runs (a broken shim, a wrong architecture).
fn find_objcopy() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(p) = env::var("LLVM_OBJCOPY") {
        candidates.push(PathBuf::from(p));
    }
    candidates.push(PathBuf::from("llvm-objcopy"));
    candidates.push(PathBuf::from("objcopy"));
    candidates.push(PathBuf::from("/opt/homebrew/opt/llvm/bin/llvm-objcopy"));
    candidates.push(PathBuf::from("/usr/local/opt/llvm/bin/llvm-objcopy"));
    candidates.into_iter().find(|c| {
        Command::new(c)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

/// The submodule's HEAD commit, or `None` when git is unavailable (then the
/// stamp records "unknown" and only a missing archive triggers a build).
fn submodule_sha(c_root: &Path) -> Option<String> {
    let out = Command::new("git")
        .args(["-C", &c_root.display().to_string(), "rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Make `variant.lib_dir/libSvtAv1Enc.a` current for `sha`, building only when
/// needed. Decision table:
/// * archive present, stamp == expected -> nothing to do;
/// * archive present, NO stamp -> a hand-built (pre-stamp) oracle: trusted as
///   is, and stamped with the current SHA so the next submodule move rebuilds;
/// * archive present, stamp differs -> `cmake --build` (incremental);
/// * archive missing -> toolchain check, configure (if no CMakeCache), build.
fn ensure_variant(c_root: &Path, v: &Variant, sha: Option<&str>) {
    let archive = v.lib_dir.join("libSvtAv1Enc.a");
    let stamp_path = v.lib_dir.join(STAMP_FILE);
    let expected = format!("{} {}\n", sha.unwrap_or("unknown-sha"), v.config_key());
    if stamp_path.exists() {
        println!("cargo:rerun-if-changed={}", stamp_path.display());
    }

    if archive.exists() {
        match fs::read_to_string(&stamp_path) {
            Ok(have) if have == expected => return,
            Ok(_) => {
                println!(
                    "cargo:warning=C reference {} is stale vs the submodule ({}); rebuilding",
                    v.name,
                    sha.unwrap_or("unknown-sha")
                );
            }
            Err(_) => {
                // Pre-stamp hand build (CI's cmake step, or a developer's own).
                // Trust it — the config it was built with is the one every
                // tool assumes — and stamp it so future submodule moves are
                // detected. Rebuilding here would double CI's C build time.
                if let Err(e) = fs::write(&stamp_path, &expected) {
                    println!(
                        "cargo:warning=could not write {}: {e} (the oracle will be re-checked \
                         on every build)",
                        stamp_path.display()
                    );
                }
                return;
            }
        }
    }

    check_toolchain();
    configure_and_build(c_root, v);
    if !archive.exists() {
        panic!(
            "cmake reported success but {} was not produced (variant: {}). See {}",
            archive.display(),
            v.name,
            v.build_dir.join("zenav1-cref-build.log").display()
        );
    }
    fs::create_dir_all(&v.lib_dir).ok();
    fs::write(&stamp_path, &expected).unwrap_or_else(|e| {
        panic!("could not write {}: {e}", stamp_path.display());
    });
    println!("cargo:rerun-if-changed={}", stamp_path.display());
}

fn check_toolchain() {
    let have = |tool: &str| {
        Command::new(tool)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    let mut missing = Vec::new();
    if !have("cmake") {
        missing.push("cmake");
    }
    // nasm assembles the x86 kernels; the arm64 build has none.
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if (arch == "x86_64" || arch == "x86") && !have("nasm") {
        missing.push("nasm");
    }
    if !have(&env::var("CC").unwrap_or_else(|_| "cc".to_string())) {
        missing.push("cc (a C compiler)");
    }
    if !missing.is_empty() {
        panic!(
            "cannot build the C reference: missing {}.\n\
             Install one-liners:\n  \
             debian/ubuntu: sudo apt-get install -y cmake nasm ninja-build build-essential\n  \
             macOS:         brew install cmake nasm ninja\n\
             (or set SVT_CREF_LIB_DIR to a directory holding a prebuilt libSvtAv1Enc.a)",
            missing.join(", ")
        );
    }
}

fn configure_and_build(c_root: &Path, v: &Variant) {
    let log_path = v.build_dir.join("zenav1-cref-build.log");
    fs::create_dir_all(&v.build_dir).unwrap_or_else(|e| {
        panic!("could not create {}: {e}", v.build_dir.display());
    });
    // CMAKE_OUTPUT_DIRECTORY needs the trailing separator (the CMakeLists
    // concatenates onto it); the same spelling every shell tool uses.
    let out_dir = format!("{}/", v.lib_dir.display());

    if !v.build_dir.join("CMakeCache.txt").exists() {
        let mut cfg = Command::new("cmake");
        cfg.arg("-S")
            .arg(c_root)
            .arg("-B")
            .arg(&v.build_dir)
            .arg("-DCMAKE_BUILD_TYPE=Release")
            .arg(format!("-DCMAKE_OUTPUT_DIRECTORY={out_dir}"))
            .arg("-DBUILD_SHARED_LIBS=OFF")
            .arg(format!(
                "-DBUILD_APPS={}",
                if v.build_apps { "ON" } else { "OFF" }
            ))
            .arg("-DBUILD_TESTING=OFF")
            // LTO changes codegen; a bit-identity oracle must not differ from
            // its counterpart by optimisation level (capture_c_trace/build.sh).
            .arg("-DSVT_AV1_LTO=OFF")
            // -march=native would pin the ISA at compile time and defeat the
            // runtime RTCD dispatch the port is compared against.
            .arg("-DNATIVE=OFF")
            .arg(format!(
                "-DSVT_HDR_MODE={}",
                if v.hdr_mode { "ON" } else { "OFF" }
            ));
        if Command::new("ninja").arg("--version").output().is_ok() {
            cfg.args(["-G", "Ninja"]);
        }
        run_logged(cfg, &log_path, "cmake configure", v);
    }

    let jobs = env::var("SVT_CREF_JOBS")
        .ok()
        .or_else(|| env::var("NUM_JOBS").ok())
        .unwrap_or_else(|| "4".to_string());
    let mut build = Command::new("cmake");
    build
        .arg("--build")
        .arg(&v.build_dir)
        .arg("--parallel")
        .arg(&jobs);
    println!(
        "cargo:warning=building the C reference {} into {} (-j{jobs}; once per submodule \
         commit, log: {})",
        v.name,
        v.build_dir.display(),
        log_path.display()
    );
    run_logged(build, &log_path, "cmake --build", v);
}

fn run_logged(mut cmd: Command, log_path: &Path, what: &str, v: &Variant) {
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("{what} for {} could not start: {e}", v.name));
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .ok();
    if let Some(f) = log.as_mut() {
        use std::io::Write;
        let _ = writeln!(f, "==== {what}: {:?}", cmd);
        let _ = f.write_all(&out.stdout);
        let _ = f.write_all(&out.stderr);
    }
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(40).collect::<Vec<_>>();
        panic!(
            "{what} FAILED for the C reference {} ({}).\nLast lines:\n{}\nFull log: {}",
            v.name,
            out.status,
            tail.into_iter().rev().collect::<Vec<_>>().join("\n"),
            log_path.display()
        );
    }
}
