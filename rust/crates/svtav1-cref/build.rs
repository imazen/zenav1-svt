//! Build script: compiles the C shims and links the in-tree C SVT-AV1 static
//! library so tests can compare Rust output against the reference bit-for-bit.

use std::env;
use std::path::PathBuf;

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

    let lib_dir = env::var("SVT_CREF_LIB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| repo_root.join("Bin/Release"));
    let archive = lib_dir.join("libSvtAv1Enc.a");
    if !archive.exists() {
        panic!(
            "libSvtAv1Enc.a not found at {}.\n\
             Build the C reference first:\n\
             cmake -S {croot} -B {root}/cbuild-static -DCMAKE_OUTPUT_DIRECTORY={root}/Bin/Release/ -DCMAKE_BUILD_TYPE=Release \
             -DBUILD_SHARED_LIBS=OFF -DBUILD_APPS=OFF -DBUILD_TESTING=OFF -DSVT_AV1_LTO=OFF && \
             cmake --build {root}/cbuild-static -j\n\
             (or set SVT_CREF_LIB_DIR to a directory containing libSvtAv1Enc.a)",
            archive.display(),
            croot = c_root.display(),
            root = repo_root.display(),
        );
    }

    println!("cargo:rerun-if-env-changed=SVT_CREF_LIB_DIR");
    println!("cargo:rerun-if-changed=shims/ref_shims.c");

    cc::Build::new()
        .file(manifest.join("shims/ref_shims.c"))
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
